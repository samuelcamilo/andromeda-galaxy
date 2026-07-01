use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ImportChecksumHistoryDTO {
    pub(crate) data: Vec<ChecksumHistoryItem>,
}

#[derive(Deserialize)]
pub struct ChecksumHistoryItem {
    pub(crate) checksum_hex: String,
    pub(crate) scam_count: u64,
    pub(crate) total_count: u64,
}

#[derive(Deserialize)]
pub struct ImportAnnotationsDTO {
    pub(crate) data: Vec<AnnotationItem>,
}

#[derive(Deserialize)]
pub struct AnnotationItem {
    pub(crate) checksum: String,
    pub(crate) text: String,
}

#[derive(Deserialize)]
pub struct GetLabelsFromAddressDTO {
    pub(crate) address: String,
}

#[derive(Deserialize)]
pub struct ChecksumCountDTO {
    pub(crate) field_name: String,
    pub(crate) field_value: String,
    pub(crate) check_rug: Option<bool>,
}

#[derive(Serialize, Clone, Deserialize, Debug)]
pub struct InsertSignatureDTO {
    pub(crate) hex_signature: String,
    pub(crate) text_signature: String,
    pub(crate) timestamp: u64,
}

#[derive(Serialize, Clone, Deserialize, Debug)]
pub struct GetSignaturesDTO {
    pub(crate) list_id: Vec<String>,
}

#[derive(Serialize, Clone, Deserialize, PartialEq, Debug)]
pub struct InsertChecksumDTO {
    pub(crate) address: String,
    pub(crate) network_id: i32,

    #[serde(flatten)]
    pub(crate) other_fields: HashMap<String, serde_json::Value>,
}
