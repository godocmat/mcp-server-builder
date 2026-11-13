//! Shared authorization and evidence primitives for preview and generated runtimes.
//!
//! Only trusted launch, transport and adapter code should construct a principal or
//! supply resolved target facts. Portable grants describe requested access.

mod grants;
mod limits;
mod paths;
mod policy;
mod redaction;
mod targets;

pub use grants::{Grant, GrantTarget, Operation, PolicyInputError, RequestedPolicy};
pub use limits::{
    ByteBudget, ByteLimitExceeded, Effect, ExecutionCeiling, ExecutionLimits, LimitOverrides,
};
pub use policy::{
    Action, ApprovedGrant, CallerGrant, CallerIdentity, CapabilityAction, DenyReason,
    OperationContext, OperatorPolicy, Permit, Policy, Principal,
};
pub use redaction::{BufferedText, REDACTED, RedactedText, RedactionError, Redactor};
pub use targets::{
    DatabaseAccess, GitOperation, HttpMethod, HttpOrigin, OriginError, RestRoute, VerifiedTarget,
};

use serde::de::value::MapAccessDeserializer;
use serde::de::{MapAccess, Visitor};

// Serde's derived struct and tagged-enum visitors also accept positional arrays. Policy
// records must use the named fields in the portable contract.
fn deserialize_policy<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct Object<T>(std::marker::PhantomData<T>);

    impl<'de, T: serde::Deserialize<'de>> Visitor<'de> for Object<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a policy object with named fields")
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
            T::deserialize(MapAccessDeserializer::new(map))
        }
    }

    // Serde may include an invalid value or key in its error message. Semantic checks
    // provide fixed field pointers and codes after shape validation.
    deserializer
        .deserialize_map(Object(std::marker::PhantomData))
        .map_err(|_| serde::de::Error::custom("policy.invalid_shape"))
}

fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
