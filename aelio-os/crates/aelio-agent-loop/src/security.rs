use crate::{canonical_hash, EffectClass, ManifestError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionBinding {
    pub tenant_id: String,
    pub conversation_id_hash: String,
    pub tool: String,
    pub tool_version: String,
    pub arguments: Value,
    pub effect_class: EffectClass,
    pub expires_at_ms: i64,
}

impl ActionBinding {
    pub fn digest(&self) -> Result<String, ManifestError> {
        canonical_hash(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationStatus {
    Pending,
    Approved,
    Denied,
    Consumed,
    Expired,
    Voided,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfirmationRecord {
    pub id: String,
    pub action_digest: String,
    pub binding: ActionBinding,
    pub status: ConfirmationStatus,
    pub created_at_ms: i64,
    pub consumed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Allow,
    Confirm { reason: String },
    Deny { reason: String },
}

/// Compose independent policy decisions using the invariant deny > confirm > allow.
/// An empty policy set is default-deny.
pub fn compose_policy(verdicts: impl IntoIterator<Item = PolicyVerdict>) -> PolicyVerdict {
    let mut saw_allow = false;
    let mut confirmation = None;
    for verdict in verdicts {
        match verdict {
            PolicyVerdict::Deny { reason } => return PolicyVerdict::Deny { reason },
            PolicyVerdict::Confirm { reason } => confirmation = Some(reason),
            PolicyVerdict::Allow => saw_allow = true,
        }
    }
    if let Some(reason) = confirmation {
        PolicyVerdict::Confirm { reason }
    } else if saw_allow {
        PolicyVerdict::Allow
    } else {
        PolicyVerdict::Deny {
            reason: "No admitted policy allows this action.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding(arguments: Value) -> ActionBinding {
        ActionBinding {
            tenant_id: "tenant-a".to_string(),
            conversation_id_hash: "conversation-hash".to_string(),
            tool: "transfer".to_string(),
            tool_version: "3".to_string(),
            arguments,
            effect_class: EffectClass::Financial,
            expires_at_ms: 123_456,
        }
    }

    #[test]
    fn action_digest_is_canonical_and_binds_arguments() {
        let first = binding(json!({"to":"alice","amount":10}));
        let reordered = binding(json!({"amount":10,"to":"alice"}));
        let altered = binding(json!({"amount":11,"to":"alice"}));
        assert_eq!(first.digest().unwrap(), reordered.digest().unwrap());
        assert_ne!(first.digest().unwrap(), altered.digest().unwrap());
    }

    #[test]
    fn policy_composition_is_deny_then_confirm_then_allow_and_defaults_deny() {
        assert!(matches!(compose_policy([]), PolicyVerdict::Deny { .. }));
        assert_eq!(compose_policy([PolicyVerdict::Allow]), PolicyVerdict::Allow);
        assert!(matches!(
            compose_policy([
                PolicyVerdict::Allow,
                PolicyVerdict::Confirm {
                    reason: "confirm".to_string()
                }
            ]),
            PolicyVerdict::Confirm { .. }
        ));
        assert!(matches!(
            compose_policy([
                PolicyVerdict::Confirm {
                    reason: "confirm".to_string()
                },
                PolicyVerdict::Deny {
                    reason: "deny".to_string()
                }
            ]),
            PolicyVerdict::Deny { .. }
        ));
    }
}
