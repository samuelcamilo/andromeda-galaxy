use std::sync::Arc;
use ethers::prelude::{Provider, TransactionReceipt, Ws};
use crate::services::ethers::find_deploys::find_deploys_service::{FindDeploysPayload, FindDeploysService};

pub struct FindOnReceipt {}

impl FindOnReceipt {
    pub async fn exec(
        provider: Arc<Provider<Ws>>,
        receipt: TransactionReceipt,
    ) -> Option<FindDeploysPayload> {
        if (receipt.contract_address.is_some()) {
            let contract_address = receipt.contract_address.unwrap();
            return FindDeploysService::validate_and_create_payload(provider, contract_address, &receipt,None).await;
        }

        None
    }

}