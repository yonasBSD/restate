// Copyright (c) 2023 - 2025 Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use std::sync::Arc;

use tokio::sync::oneshot;
use tracing::trace;

use restate_core::{ShutdownError, cancellation_token};
use restate_metadata_store::ReadModifyWriteError;
use restate_types::logs::builder::LogsBuilder;
use restate_types::logs::metadata::{LogletParams, Logs, ProviderKind, SealMetadata, SegmentIndex};
use restate_types::logs::{LogId, Lsn};

use crate::Error;
use crate::bifrost::{BifrostInner, ExtendLogChainReceiver};
use crate::error::AdminError;

const MAX_BATCH_SIZE_LOG_CHAIN_EXTENSIONS: usize = 128;

struct OpOutput<T> {
    tx: oneshot::Sender<Result<T, Error>>,
    staged_result: Option<Result<T, Error>>,
}

impl<T> OpOutput<T> {
    fn stage_output(&mut self, result: Result<T, Error>) {
        self.staged_result = Some(result);
    }

    fn fail(self, err: Error) {
        // ignore if the receiver disappeared
        let _ = self.tx.send(Err(err));
    }

    fn complete(self) {
        let result = self
            .staged_result
            .expect("complete must be called on a staged result");
        // ignore if the receiver disappeared
        let _ = self.tx.send(result);
    }
}

pub(super) struct LogChainCommand {
    log_id: LogId,
    last_segment_index: SegmentIndex,
    op: ChainOp,
}

impl LogChainCommand {
    pub fn extend(
        log_id: LogId,
        last_segment_index: SegmentIndex,
        base_lsn: Lsn,
        provider: ProviderKind,
        params: LogletParams,
    ) -> (oneshot::Receiver<Result<(), Error>>, Self) {
        let (tx, rx) = oneshot::channel();
        let cmd = Self {
            log_id,
            last_segment_index,
            op: ChainOp::Extend {
                base_lsn,
                provider,
                params,
                response: OpOutput {
                    tx,
                    staged_result: None,
                },
            },
        };
        (rx, cmd)
    }

    pub fn seal_chain(
        log_id: LogId,
        last_segment_index: SegmentIndex,
        tail_lsn: Lsn,
        metadata: SealMetadata,
    ) -> (oneshot::Receiver<Result<Lsn, Error>>, Self) {
        let (tx, rx) = oneshot::channel();
        let cmd = Self {
            log_id,
            last_segment_index,
            op: ChainOp::SealChain {
                tail_lsn,
                metadata,
                response: OpOutput {
                    tx,
                    staged_result: None,
                },
            },
        };
        (rx, cmd)
    }

    fn fail(self, err: Error) {
        match self.op {
            ChainOp::Extend { response, .. } => response.fail(err),
            ChainOp::SealChain { response, .. } => response.fail(err),
        }
    }

    fn complete(self) {
        match self.op {
            ChainOp::Extend { response, .. } => response.complete(),
            ChainOp::SealChain { response, .. } => response.complete(),
        }
    }
}

enum ChainOp {
    Extend {
        base_lsn: Lsn,
        provider: ProviderKind,
        params: LogletParams,
        response: OpOutput<()>,
    },
    SealChain {
        tail_lsn: Lsn,
        metadata: SealMetadata,
        response: OpOutput<Lsn>,
    },
}

/// Component which coalesces multiple log-chain updates into a single [`Logs`] update. It works
/// by draining all available [`LogChainCommand`] commands and applying them to the current logs
/// configuration using a read-modify-write metadata operation. A log chain can only be extended if
/// the last segment index equals the value specified by the [`LogChainCommand`] command.
pub struct LogChainExtender {
    inner: Arc<BifrostInner>,
    extend_log_chain_rx: ExtendLogChainReceiver,
}

impl LogChainExtender {
    pub fn new(inner: Arc<BifrostInner>, extend_log_chain_rx: ExtendLogChainReceiver) -> Self {
        Self {
            inner,
            extend_log_chain_rx,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        trace!("Bifrost log chain extender started");

        cancellation_token()
            .run_until_cancelled(self.run_inner())
            .await
            .ok_or(ShutdownError)?;

        Ok(())
    }

    pub async fn run_inner(mut self) {
        let mut buffer = Vec::new();

        // await the first log chain command
        loop {
            let received = self
                .extend_log_chain_rx
                .recv_many(&mut buffer, MAX_BATCH_SIZE_LOG_CHAIN_EXTENSIONS)
                .await;

            if received == 0 {
                break;
            }

            // batch-apply all collected log chain commands
            match self
                .inner
                .metadata_writer
                .global_metadata()
                .read_modify_write(|logs: Option<Arc<Logs>>| {
                    let mut builder =
                        Arc::unwrap_or_clone(logs.ok_or(Error::LogsMetadataNotProvisioned)?)
                            .into_builder();

                    for cmd in &mut buffer {
                        match cmd.op {
                            ChainOp::Extend {
                                base_lsn,
                                provider,
                                ref params,
                                ref mut response,
                            } => {
                                response.stage_output(Self::extend_log_chain(
                                    &mut builder,
                                    cmd.log_id,
                                    cmd.last_segment_index,
                                    base_lsn,
                                    provider,
                                    params,
                                ));
                            }
                            ChainOp::SealChain {
                                tail_lsn,
                                ref metadata,
                                ref mut response,
                            } => {
                                response.stage_output(Self::seal_log_chain(
                                    &mut builder,
                                    cmd.log_id,
                                    cmd.last_segment_index,
                                    tail_lsn,
                                    metadata,
                                ));
                            }
                        }
                    }

                    Ok(builder.build())
                })
                .await
                .map_err(|err: ReadModifyWriteError<Error>| err.transpose())
            {
                Ok(_) => {
                    for cmd in buffer.drain(..) {
                        cmd.complete();
                    }
                }
                Err(err) => {
                    for cmd in buffer.drain(..) {
                        cmd.fail(err.clone());
                    }
                }
            }
        }
    }

    fn extend_log_chain(
        builder: &mut LogsBuilder,
        log_id: LogId,
        last_segment_index: SegmentIndex,
        base_lsn: Lsn,
        provider_kind: ProviderKind,
        params: &LogletParams,
    ) -> Result<(), Error> {
        let mut chain_builder = builder.chain(log_id).ok_or(Error::UnknownLogId(log_id))?;

        if chain_builder.tail().index() != last_segment_index {
            // tail is not what we expected.
            Err(AdminError::SegmentMismatch {
                expected: last_segment_index,
                found: chain_builder.tail().index(),
            })?;
        }

        let _ = chain_builder
            .append_segment(base_lsn, provider_kind, params.clone())
            .map_err(AdminError::from)?;

        Ok(())
    }

    fn seal_log_chain(
        builder: &mut LogsBuilder,
        log_id: LogId,
        last_segment_index: SegmentIndex,
        tail_lsn: Lsn,
        metadata: &SealMetadata,
    ) -> Result<Lsn, Error> {
        let mut chain_builder = builder.chain(log_id).ok_or(Error::UnknownLogId(log_id))?;

        if chain_builder.tail().index() != last_segment_index {
            // tail segment is not what we expected.
            Err(AdminError::SegmentMismatch {
                expected: last_segment_index,
                found: chain_builder.tail().index(),
            })?;
        }

        let lsn = chain_builder
            .seal(tail_lsn, metadata)
            .map_err(AdminError::from)?;

        Ok(lsn)
    }
}
