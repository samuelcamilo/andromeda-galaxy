use crate::repositories::sqlite_repository::SqliteRepository;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct ElasticLabel {
    pub address: String,
    #[serde(rename = "chainId")]
    pub chain_id: u32,
    pub label: String,
    #[serde(rename = "nameTag")]
    pub name_tag: String,
}

pub struct LabelsService {
    repository: Arc<SqliteRepository>,
}

impl LabelsService {
    pub fn new(repository: Arc<SqliteRepository>) -> Self {
        LabelsService { repository }
    }

    pub async fn exec_by_address(
        &self,
        address: String,
    ) -> Result<Vec<ElasticLabel>, Box<dyn std::error::Error>> {
        let rows = self.repository.search_labels_by_address(&address)?;

        let labels = rows
            .into_iter()
            .map(|r| ElasticLabel {
                address: r.address,
                chain_id: r.chain_id,
                label: r.label,
                name_tag: r.name_tag,
            })
            .collect();

        Ok(labels)
    }
}
