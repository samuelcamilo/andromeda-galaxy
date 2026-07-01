use crate::controllers::dto::elastic_dto::{ChecksumCountDTO, ImportAnnotationsDTO, ImportChecksumHistoryDTO, InsertChecksumDTO};
use crate::repositories::sqlite_repository::SqliteRepository;
use std::sync::Arc;

pub struct ChecksumService {
    repository: Arc<SqliteRepository>,
}

impl ChecksumService {
    pub fn new(repository: Arc<SqliteRepository>) -> Self {
        ChecksumService { repository }
    }

    pub async fn update_if_necessary(&self, dto: &InsertChecksumDTO) {
        let existing = self.repository.get_checksum(&dto.address);

        if let Ok(Some(row)) = existing {
            let current_extra: serde_json::Value =
                serde_json::from_str(&row.extra_fields).unwrap_or_default();
            let new_extra = serde_json::Value::Object(
                dto.other_fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );

            if row.network_id != dto.network_id || current_extra != new_extra {
                self.upsert(dto).await;
            }
        }
    }

    pub async fn upsert(&self, dto: &InsertChecksumDTO) {
        let extra = serde_json::to_string(&dto.other_fields).unwrap_or_default();
        self.repository
            .upsert_checksum(&dto.address, dto.network_id, &extra)
            .expect("Erro ao fazer upsert de checksum");
    }

    pub async fn checksum_count(&self, dto: &ChecksumCountDTO) -> u64 {
        self.repository
            .checksum_count(&dto.field_name, &dto.field_value, dto.check_rug)
            .unwrap_or(0)
    }

    pub async fn import_checksum_history(&self, dto: &ImportChecksumHistoryDTO) -> usize {
        let mut count = 0;
        for item in &dto.data {
            if self.repository.upsert_checksum_history(
                &item.checksum_hex, item.scam_count, item.total_count
            ).is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn clear_checksum_history(&self) -> usize {
        self.repository.clear_checksum_history().unwrap_or(0)
    }

    pub async fn import_annotations(&self, dto: &ImportAnnotationsDTO) -> usize {
        let mut count = 0;
        for item in &dto.data {
            if self.repository.set_annotation(&item.checksum, &item.text).is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn import_gas_annotations(&self, dto: &ImportAnnotationsDTO) -> usize {
        let mut count = 0;
        for item in &dto.data {
            if self.repository.set_gas_annotation(&item.checksum, &item.text).is_ok() {
                count += 1;
            }
        }
        count
    }

    pub async fn import_indicators(&self, dto: &ImportAnnotationsDTO) -> usize {
        let mut count = 0;
        for item in &dto.data {
            if self.repository.set_indicator(&item.checksum, &item.text).is_ok() {
                count += 1;
            }
        }
        count
    }
}
