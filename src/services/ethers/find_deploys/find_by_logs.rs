use crate::services::ethers::find_deploys::find_deploys_service::{FindDeploysPayload, FindDeploysService};
use ethers::addressbook::Address;
use ethers::prelude::{Provider, TransactionReceipt, Ws, H256};
use std::sync::Arc;

pub struct FindByLogs {}

impl FindByLogs {
    fn topic_is_mint(topic: Vec<H256>) -> bool {
        let transfer_hash = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        let mint_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";

        topic.len() > 1
            && topic[0] == transfer_hash.parse().unwrap()
            && topic[1] == mint_hash.parse().unwrap()
    }

    fn topic_is_transfer_ownership(topic: Vec<H256>, from: Address) -> bool {
        let transfer_ownership_hash =
            "0x8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0";

        let padded_str = format!("0x{:0>64}", format!("{:x}", from));

        topic.len() > 1
            && topic[0] == transfer_ownership_hash.parse().unwrap()
        // && topic[2] == padded_str.parse().unwrap()
    }

    fn topic_is_transfer_from(topic: Vec<H256>, from: String) -> bool {
        let transfer_hash = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        // let from_hash = H256::from_str(&*from.as_str().to_lowercase()).unwrap();

        topic.len() > 1 &&
            topic[0] == transfer_hash.parse().unwrap()
        // && topic[2] == from_hash
    }

    pub async fn exec(
        provider: Arc<Provider<Ws>>,
        receipt: TransactionReceipt,
    ) -> Option<FindDeploysPayload> {
        let has_transfer_ownership = receipt.logs.iter().any(|log| {
            Self::topic_is_transfer_ownership(log.topics.clone(), receipt.from)
        });

        if !has_transfer_ownership {
            return None;
        }

        for log in receipt.logs.iter() {
            if Self::topic_is_mint(log.topics.clone()) {
                return FindDeploysService::validate_and_create_payload(provider, log.address, &receipt,None).await;
            }
        }

        None
    }


}