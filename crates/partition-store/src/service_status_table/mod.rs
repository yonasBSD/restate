// Copyright (c) 2023 - 2025 Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

use crate::TableScan::FullScanPartitionKeyRange;
use crate::keys::{KeyKind, TableKey, define_table_key};
use crate::owned_iter::OwnedIterator;
use crate::protobuf_types::PartitionStoreProtobufValue;
use crate::{PartitionStore, TableKind};
use crate::{PartitionStoreTransaction, StorageAccess};
use bytestring::ByteString;
use futures::Stream;
use futures_util::stream;
use restate_rocksdb::RocksDbPerfGuard;
use restate_storage_api::Result;
use restate_storage_api::service_status_table::{
    ReadOnlyVirtualObjectStatusTable, VirtualObjectStatus, VirtualObjectStatusTable,
};
use restate_types::identifiers::WithPartitionKey;
use restate_types::identifiers::{PartitionKey, ServiceId};
use std::ops::RangeInclusive;

define_table_key!(
    TableKind::ServiceStatus,
    KeyKind::ServiceStatus,
    ServiceStatusKey(
        partition_key: PartitionKey,
        service_name: ByteString,
        service_key: ByteString
    )
);

impl PartitionStoreProtobufValue for VirtualObjectStatus {
    type ProtobufType = crate::protobuf_types::v1::VirtualObjectStatus;
}

fn write_status_key(service_id: &ServiceId) -> ServiceStatusKey {
    ServiceStatusKey::default()
        .partition_key(service_id.partition_key())
        .service_name(service_id.service_name.clone())
        .service_key(service_id.key.clone())
}

fn put_virtual_object_status<S: StorageAccess>(
    storage: &mut S,
    service_id: &ServiceId,
    status: &VirtualObjectStatus,
) -> Result<()> {
    let key = ServiceStatusKey::default()
        .partition_key(service_id.partition_key())
        .service_name(service_id.service_name.clone())
        .service_key(service_id.key.clone());
    if *status == VirtualObjectStatus::Unlocked {
        storage.delete_key(&key)
    } else {
        storage.put_kv(key, status)
    }
}

fn get_virtual_object_status<S: StorageAccess>(
    storage: &mut S,
    service_id: &ServiceId,
) -> Result<VirtualObjectStatus> {
    let _x = RocksDbPerfGuard::new("get-virtual-obj-status");
    let key = ServiceStatusKey::default()
        .partition_key(service_id.partition_key())
        .service_name(service_id.service_name.clone())
        .service_key(service_id.key.clone());

    storage
        .get_value(key)
        .map(|value| value.unwrap_or(VirtualObjectStatus::Unlocked))
}

fn all_virtual_object_status<S: StorageAccess>(
    storage: &S,
    range: RangeInclusive<PartitionKey>,
) -> Result<impl Stream<Item = Result<(ServiceId, VirtualObjectStatus)>> + Send + use<'_, S>> {
    let iter = storage.iterator_from(FullScanPartitionKeyRange::<ServiceStatusKey>(range))?;
    Ok(stream::iter(OwnedIterator::new(iter).map(
        |(mut key, mut value)| {
            let state_key = ServiceStatusKey::deserialize_from(&mut key)?;
            let state_value = VirtualObjectStatus::decode(&mut value)?;

            let (partition_key, service_name, service_key) = state_key.into_inner_ok_or()?;

            Ok((
                ServiceId::from_parts(partition_key, service_name, service_key),
                state_value,
            ))
        },
    )))
}

fn delete_virtual_object_status<S: StorageAccess>(
    storage: &mut S,
    service_id: &ServiceId,
) -> Result<()> {
    let key = write_status_key(service_id);
    storage.delete_key(&key)
}

impl ReadOnlyVirtualObjectStatusTable for PartitionStore {
    async fn get_virtual_object_status(
        &mut self,
        service_id: &ServiceId,
    ) -> Result<VirtualObjectStatus> {
        self.assert_partition_key(service_id)?;
        get_virtual_object_status(self, service_id)
    }

    fn all_virtual_object_statuses(
        &self,
        range: RangeInclusive<PartitionKey>,
    ) -> Result<impl Stream<Item = Result<(ServiceId, VirtualObjectStatus)>> + Send> {
        all_virtual_object_status(self, range)
    }
}

impl ReadOnlyVirtualObjectStatusTable for PartitionStoreTransaction<'_> {
    async fn get_virtual_object_status(
        &mut self,
        service_id: &ServiceId,
    ) -> Result<VirtualObjectStatus> {
        self.assert_partition_key(service_id)?;
        get_virtual_object_status(self, service_id)
    }

    fn all_virtual_object_statuses(
        &self,
        range: RangeInclusive<PartitionKey>,
    ) -> Result<impl Stream<Item = Result<(ServiceId, VirtualObjectStatus)>> + Send> {
        all_virtual_object_status(self, range)
    }
}

impl VirtualObjectStatusTable for PartitionStoreTransaction<'_> {
    async fn put_virtual_object_status(
        &mut self,
        service_id: &ServiceId,
        status: &VirtualObjectStatus,
    ) -> Result<()> {
        self.assert_partition_key(service_id)?;
        put_virtual_object_status(self, service_id, status)
    }

    async fn delete_virtual_object_status(&mut self, service_id: &ServiceId) -> Result<()> {
        self.assert_partition_key(service_id)?;
        delete_virtual_object_status(self, service_id)
    }
}
