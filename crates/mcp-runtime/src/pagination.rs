use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use uuid::Uuid;

use crate::{ErrorKind, Page, Principal};

const MAX_PAGE_ITEMS: usize = 32;
const MAX_PAGE_BYTES: usize = 1024 * 1024 - 4096;
const MAX_CURSORS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListKind {
    Tools,
    Resources,
    ResourceTemplates,
    Prompts,
}

enum Caller {
    Anonymous,
    Local(String),
    Remote {
        issuer: String,
        subject: String,
        client_id: Option<String>,
    },
}

impl Caller {
    fn new(principal: &Principal<'_>) -> Option<Self> {
        fn bounded(value: &str) -> Option<String> {
            (value.len() <= 2048).then(|| value.to_owned())
        }
        Some(match principal {
            Principal::Anonymous => Self::Anonymous,
            Principal::Local { operator } => Self::Local(bounded(operator)?),
            Principal::Remote {
                issuer,
                subject,
                client_id,
                ..
            } => Self::Remote {
                issuer: bounded(issuer)?,
                subject: bounded(subject)?,
                client_id: match client_id {
                    Some(value) => Some(bounded(value)?),
                    None => None,
                },
            },
        })
    }

    fn matches(&self, principal: &Principal<'_>) -> bool {
        match (self, principal) {
            (Self::Anonymous, Principal::Anonymous) => true,
            (Self::Local(identity), Principal::Local { operator }) => identity == operator,
            (
                Self::Remote {
                    issuer,
                    subject,
                    client_id,
                },
                Principal::Remote {
                    issuer: actual_issuer,
                    subject: actual_subject,
                    client_id: actual_client,
                    ..
                },
            ) => {
                issuer == actual_issuer
                    && subject == actual_subject
                    && client_id.as_deref() == *actual_client
            }
            _ => false,
        }
    }
}

struct Cursor {
    token: Uuid,
    caller: Caller,
    kind: ListKind,
    visible: Vec<usize>,
    offset: usize,
}

#[derive(Default)]
pub(crate) struct Pagination(Mutex<VecDeque<Cursor>>);

impl Pagination {
    pub(crate) fn page<'a, T: Serialize>(
        &self,
        kind: ListKind,
        principal: &Principal<'_>,
        cursor: Option<&str>,
        visible: Vec<(usize, &'a T)>,
    ) -> Result<Page<'a, T>, ErrorKind> {
        let visibility: Vec<_> = visible.iter().map(|(index, _)| *index).collect();
        let offset = if let Some(token) = cursor {
            let token = Uuid::parse_str(token).map_err(|_| ErrorKind::InvalidCursor)?;
            self.0
                .lock()
                .expect("pagination lock")
                .iter()
                .find(|entry| {
                    entry.token == token
                        && entry.kind == kind
                        && entry.caller.matches(principal)
                        && entry.visible == visibility
                })
                .ok_or(ErrorKind::InvalidCursor)?
                .offset
        } else {
            0
        };

        let mut bytes = 0;
        let mut items = Vec::new();
        for (_, item) in visible.iter().skip(offset).take(MAX_PAGE_ITEMS) {
            let Some(item_bytes) =
                crate::execution::serialized_len(item, MAX_PAGE_BYTES - bytes - 1)
            else {
                if items.is_empty() {
                    return Err(ErrorKind::OutputLimit);
                }
                break;
            };
            bytes += item_bytes + 1;
            items.push(*item);
            if bytes == MAX_PAGE_BYTES {
                break;
            }
        }

        let next = offset + items.len();
        let next_cursor = if next < visible.len() {
            let mut cursors = self.0.lock().expect("pagination lock");
            if let Some(existing) = cursors.iter().find(|entry| {
                entry.offset == next
                    && entry.kind == kind
                    && entry.caller.matches(principal)
                    && entry.visible == visibility
            }) {
                Some(existing.token.to_string())
            } else {
                if cursors.len() == MAX_CURSORS {
                    cursors.pop_front();
                }
                let token = Uuid::new_v4();
                cursors.push_back(Cursor {
                    token,
                    caller: Caller::new(principal).ok_or(ErrorKind::InvalidCursor)?,
                    kind,
                    visible: visibility,
                    offset: next,
                });
                Some(token.to_string())
            }
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }
}
