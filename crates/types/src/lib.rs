// Copyright (c) 2023 - 2025 Restate Software, Inc., Restate GmbH.
// All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

//! This crate contains the core types used by various Restate components.

mod base62_util;
mod id_util;
mod macros;
mod node_id;
mod version;

pub mod art;
pub mod cluster;

pub mod cluster_state;
pub mod health;

pub mod config;
pub mod config_loader;
pub mod deployment;
pub mod endpoint_manifest;
pub mod epoch;
pub mod errors;
pub mod identifiers;
pub mod invocation;
pub mod journal;
pub mod journal_v2;
pub mod live;
pub mod locality;
pub mod logs;
pub mod message;
pub mod metadata;
pub mod metadata_store;
pub mod net;
pub mod nodes_config;
pub mod partition_table;
pub mod partitions;
pub mod protobuf;
pub mod replicated_loglet;
pub mod replication;
pub mod retries;
pub mod schema;
pub mod service_discovery;
pub mod service_protocol;
pub mod state_mut;
pub mod storage;
pub mod time;
pub mod timer;

use std::num::NonZero;
use std::ops::RangeInclusive;

use bilrost::encoding::{
    EmptyState, ForOverwrite, General, GeneralGeneric, ValueDecoder, ValueEncoder, Wiretyped,
};
use serde_with::serde_as;

use restate_encoding::{BilrostAs, NetSerde};

pub use id_util::{IdDecoder, IdEncoder, IdResourceType, IdStrCursor};
pub use node_id::*;
pub use version::*;

// Re-export metrics' SharedString (Space-efficient Cow + RefCounted variant)
pub type SharedString = metrics::SharedString;

/// Trait for merging two attributes
pub trait Merge {
    /// Return true if the value was mutated as a result of the merge
    fn merge(&mut self, other: Self) -> bool;
}

impl Merge for bool {
    fn merge(&mut self, other: Self) -> bool {
        if *self != other {
            *self |= other;
            true
        } else {
            false
        }
    }
}

/// A bilrost compatible wrapper to be used instead of Option<NonZero<T>> that encodes
/// as `zero` if is set to None.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct OptionNonZero<P>(Option<P>);

impl<P> Default for OptionNonZero<P> {
    fn default() -> Self {
        Self(None)
    }
}

impl<Primitive> NetSerde for OptionNonZero<Primitive> where Primitive: NetSerde {}

macro_rules! impl_option_non_zero {
    ($t:ty) => {
        impl From<OptionNonZero<$t>> for Option<NonZero<$t>> {
            fn from(value: OptionNonZero<$t>) -> Self {
                value
                    .0
                    .map(|v| NonZero::<$t>::try_from(v).expect("is non-zero"))
            }
        }

        impl From<Option<NonZero<$t>>> for OptionNonZero<$t> {
            fn from(value: Option<NonZero<$t>>) -> Self {
                Self(value.map(|v| v.get()))
            }
        }
    };
    ($($t:ty),+) => {
        $(impl_option_non_zero!($t);)+
    }
}

impl_option_non_zero!(u8, u16, u32, u64, usize);

impl<const _P: u8, P> ValueEncoder<GeneralGeneric<_P>, OptionNonZero<P>> for ()
where
    P: Default + Copy,
    (): ValueEncoder<General, P>,
{
    fn encode_value<B: ::bytes::BufMut + ?Sized>(value: &OptionNonZero<P>, buf: &mut B) {
        let value = value.0.unwrap_or_default();
        <() as ValueEncoder<General, P>>::encode_value(&value, buf)
    }

    fn value_encoded_len(value: &OptionNonZero<P>) -> usize {
        let value = value.0.unwrap_or_default();
        <() as ValueEncoder<General, P>>::value_encoded_len(&value)
    }

    fn prepend_value<B: bilrost::buf::ReverseBuf + ?Sized>(value: &OptionNonZero<P>, buf: &mut B) {
        let value = value.0.unwrap_or_default();
        <() as ValueEncoder<General, P>>::prepend_value(&value, buf)
    }
}

#[allow(clippy::all)]
impl<const _P: u8, P> ValueDecoder<GeneralGeneric<_P>, OptionNonZero<P>> for ()
where
    P: Default + Copy + PartialEq,
    (): ValueDecoder<General, P>,
{
    fn decode_value<B: ::bytes::Buf + ?Sized>(
        value: &mut OptionNonZero<P>,
        buf: ::bilrost::encoding::Capped<B>,
        ctx: ::bilrost::encoding::DecodeContext,
    ) -> ::std::result::Result<(), ::bilrost::DecodeError> {
        let zero = P::default();
        let mut inner = zero;
        <() as ValueDecoder<General, P>>::decode_value(&mut inner, buf, ctx)?;
        if inner != zero {
            value.0 = Some(inner);
        }

        Ok(())
    }
}

#[allow(clippy::all)]
impl<const _P: u8, P> Wiretyped<GeneralGeneric<_P>, OptionNonZero<P>> for ()
where
    (): Wiretyped<General, P>,
{
    const WIRE_TYPE: ::bilrost::encoding::WireType = <() as Wiretyped<General, P>>::WIRE_TYPE;
}

#[allow(clippy::all)]
impl<P> EmptyState<(), OptionNonZero<P>> for () {
    fn clear(val: &mut OptionNonZero<P>) {
        val.0 = None;
    }
    fn empty() -> OptionNonZero<P> {
        OptionNonZero::default()
    }
    fn is_empty(val: &OptionNonZero<P>) -> bool {
        val.0.is_none()
    }
}

impl<P> ForOverwrite<(), OptionNonZero<P>> for () {
    fn for_overwrite() -> OptionNonZero<P> {
        OptionNonZero::default()
    }
}

/// A newtype wrapper around [`enumset::EnumSet`] enabling
/// serialization and deserialization as a Bilrost message.
#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    BilrostAs,
    serde::Serialize,
    serde::Deserialize,
    derive_more::Deref,
    derive_more::From,
    derive_more::Into,
    derive_more::Display,
)]
#[bilrost_as(EnumSetMessage)]
pub struct NetEnumSet<T>(enumset::EnumSet<T>)
where
    T: enumset::EnumSetType + 'static;

impl<T> NetSerde for NetEnumSet<T> where T: NetSerde + enumset::EnumSetType {}

impl<T> Default for NetEnumSet<T>
where
    T: enumset::EnumSetType,
{
    fn default() -> Self {
        Self(enumset::EnumSet::empty())
    }
}

#[derive(bilrost::Message)]
struct EnumSetMessage(u64);

impl<T> From<&NetEnumSet<T>> for EnumSetMessage
where
    T: enumset::EnumSetType,
{
    fn from(value: &NetEnumSet<T>) -> Self {
        Self(value.0.as_u64())
    }
}

impl<T> From<EnumSetMessage> for NetEnumSet<T>
where
    T: enumset::EnumSetType + 'static,
{
    fn from(value: EnumSetMessage) -> Self {
        Self(enumset::EnumSet::<T>::from_u64_truncated(value.0))
    }
}

impl<T> From<T> for NetEnumSet<T>
where
    T: enumset::EnumSetType,
{
    fn from(value: T) -> Self {
        enumset::EnumSet::from(value).into()
    }
}

/// A newtype wrapper around [`RangeInclusive<Idx>`] enabling
/// serialization and deserialization as a Bilrost message.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BilrostAs,
    serde::Serialize,
    serde::Deserialize,
    derive_more::Deref,
    derive_more::From,
    derive_more::Into,
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[bilrost_as(RangeInclusiveMessage<Idx>)]
pub struct NetRangeInclusive<Idx>(RangeInclusive<Idx>)
where
    Idx: Copy + 'static,
    (): EmptyState<(), Idx> + ValueEncoder<General, Idx> + ValueDecoder<General, Idx>;

impl<Idx> Default for NetRangeInclusive<Idx>
where
    Idx: Copy + 'static,
    (): EmptyState<(), Idx> + ValueEncoder<General, Idx> + ValueDecoder<General, Idx>,
{
    fn default() -> Self {
        Self(RangeInclusive::new(
            <() as EmptyState<(), Idx>>::empty(),
            <() as EmptyState<(), Idx>>::empty(),
        ))
    }
}

#[derive(bilrost::Message)]
struct RangeInclusiveMessage<Idx>((Idx, Idx))
where
    (): EmptyState<(), Idx> + ValueEncoder<General, Idx> + ValueDecoder<General, Idx>;

impl<Idx> From<&NetRangeInclusive<Idx>> for RangeInclusiveMessage<Idx>
where
    Idx: Copy,
    (): EmptyState<(), Idx> + ValueEncoder<General, Idx> + ValueDecoder<General, Idx>,
{
    fn from(value: &NetRangeInclusive<Idx>) -> Self {
        Self((*value.0.start(), *value.0.end()))
    }
}

impl<Idx> From<RangeInclusiveMessage<Idx>> for NetRangeInclusive<Idx>
where
    Idx: Copy,
    (): EmptyState<(), Idx> + ValueEncoder<General, Idx> + ValueDecoder<General, Idx>,
{
    fn from(value: RangeInclusiveMessage<Idx>) -> Self {
        Self(RangeInclusive::new(value.0.0, value.0.1))
    }
}

/// A newtype wrapper around [`serde_json::Value`] enabling
/// serialization and deserialization as a Bilrost message.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::From,
    derive_more::Into,
    derive_more::Deref,
    derive_more::Display,
    derive_more::FromStr,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NetJsonValue(serde_json::Value);

restate_encoding::bilrost_as_display_from_str!(NetJsonValue);

impl Default for NetJsonValue {
    fn default() -> Self {
        Self(serde_json::Value::Null)
    }
}

#[serde_as]
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    derive_more::From,
    derive_more::Into,
    derive_more::Deref,
    derive_more::Display,
    derive_more::FromStr,
    serde::Serialize,
    serde::Deserialize,
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct NetHumanDuration(
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    humantime::Duration,
);

restate_encoding::bilrost_as_display_from_str!(NetHumanDuration);

impl Default for NetHumanDuration {
    fn default() -> Self {
        Self(humantime::Duration::from(std::time::Duration::from_secs(0)))
    }
}

impl From<std::time::Duration> for NetHumanDuration {
    fn from(value: std::time::Duration) -> Self {
        Self(humantime::Duration::from(value))
    }
}

impl From<NetHumanDuration> for std::time::Duration {
    fn from(value: NetHumanDuration) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod test {
    use bilrost::{Message, OwnedMessage};
    use bytes::BytesMut;

    use crate::NetJsonValue;

    #[test]
    fn json_value() {
        #[derive(bilrost::Message)]
        struct Message {
            payload: NetJsonValue,
        }

        let m = Message {
            payload: serde_json::json!({
                "hello": "World",
            })
            .into(),
        };

        let mut buf = BytesMut::new();
        m.encode(&mut buf).unwrap();

        let loaded = Message::decode(buf).unwrap();

        assert_eq!(m.payload.0, loaded.payload.0);
    }
}
