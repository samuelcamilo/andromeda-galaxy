use std::collections::HashMap;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct RecallSomebodyWebhookDTO {
    pub(crate) body:HashMap<Value,Value>,
    pub(crate) webhook:String,
    pub(crate) timer:u64,
    pub(crate) identifier:String
}
