use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::Diagnostic;

#[derive(Debug)]
pub enum Reply<T> {
    Success {
        data: T,
        diagnostics: Vec<Diagnostic>,
    },
    Failure {
        error: Diagnostic,
        diagnostics: Vec<Diagnostic>,
    },
}

impl<T: Serialize> Serialize for Reply<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The wire discriminator is a JSON boolean.
        let mut reply = serializer.serialize_struct("Reply", 3)?;
        match self {
            Self::Success { data, diagnostics } => {
                reply.serialize_field("ok", &true)?;
                reply.serialize_field("data", data)?;
                reply.serialize_field("diagnostics", diagnostics)?;
            }
            Self::Failure { error, diagnostics } => {
                reply.serialize_field("ok", &false)?;
                reply.serialize_field("error", error)?;
                reply.serialize_field("diagnostics", diagnostics)?;
            }
        }
        reply.end()
    }
}
