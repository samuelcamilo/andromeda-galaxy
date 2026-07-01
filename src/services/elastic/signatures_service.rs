use crate::controllers::dto::elastic_dto::{GetSignaturesDTO, InsertSignatureDTO};
use crate::repositories::sqlite_repository::SqliteRepository;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct SignaturesService {
    repository: Arc<SqliteRepository>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SignaturesByIdResponse {
    pub hex_signature: String,
    pub text_signature: String,
    pub timestamp: f64,
}

impl SignaturesService {
    pub fn new(repository: Arc<SqliteRepository>) -> Self {
        SignaturesService { repository }
    }

    pub async fn get_signatures_by_id(&self, dto: &GetSignaturesDTO) -> Vec<SignaturesByIdResponse> {
        self.repository
            .get_signatures_by_ids(&dto.list_id)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SignaturesByIdResponse {
                hex_signature: r.hex_signature,
                text_signature: r.text_signature,
                timestamp: r.timestamp,
            })
            .collect()
    }

    pub async fn upsert(&self, dto: &InsertSignatureDTO) {
        self.repository
            .upsert_signature(&dto.hex_signature, &dto.text_signature, dto.timestamp as f64)
            .expect("Erro ao fazer upsert de signature");
    }
}
