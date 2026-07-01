use ethers::types::TransactionRequest;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct ApplyRpcDTO {
    pub(crate) endpoint: String,
    pub(crate) listen_contract_event: Option<bool>,
    pub(crate) listen_deploy_event: Option<bool>,
    pub(crate) identifier: String,
}

#[derive(Deserialize, Serialize)]
pub struct ApplyForkDTO {
    pub(crate) endpoint: String,
    pub(crate) block_number: Option<u64>,
    pub(crate) identifier: String,
}

#[derive(Deserialize, Serialize)]
pub struct SimulateTxDTO {
    pub(crate) transaction: TransactionRequest,
    pub(crate) identifier: String,
    pub(crate) block_number: Option<u64>,
}

#[derive(Deserialize, Serialize)]
pub struct SetBalanceDTO {
    pub(crate) address: String,
    pub(crate) identifier: String,
}

#[derive(Deserialize, Serialize)]
pub struct AnvilGetTransactionCount {
    pub(crate) address: String,
    pub(crate) identifier: String,
}

#[derive(Deserialize, Serialize)]
pub struct RemoveDTO {
    pub(crate) identifier: String,
}

#[derive(Deserialize, Serialize)]
pub struct CallTxDto {
    pub(crate) contract_address: String,
    pub(crate) method: String,
    pub(crate) args: Vec<String>,
}

#[derive(Deserialize)]
pub struct CallFunctionsDTO {
    pub(crate) functions_name: Vec<String>,
    pub(crate) abi: String,
    pub(crate) address: String,
}

#[derive(Deserialize)]
pub struct GetLogsDTO {
    pub(crate) from_block: u64,
    pub(crate) to_block: u64,
}

#[derive(Deserialize)]
pub struct GetCodeDTO {
    pub(crate) address: String,
}

#[derive(Deserialize)]
pub struct GetTransactionCountDTO {
    pub(crate) address: String,
    pub(crate) block_number: Option<u64>,
}

#[derive(Deserialize)]
pub struct ListenContractEventsDTO {
    pub(crate) address: String,
    pub(crate) webhook: String,
    pub(crate) event_signature: String,
}

#[derive(Deserialize)]
pub struct ListenDeployErc20ContractsDTO {
    pub(crate) webhook: String,
}

#[derive(Deserialize)]
pub struct GetTransactionDTO {
    pub(crate) transaction_hash: String,
}

#[derive(Deserialize)]
pub struct CallSelectorsDTO {
    pub(crate) address: String,
    pub(crate) selectors_id: Vec<String>,
}

#[derive(Deserialize)]
pub struct PathParams {
    pub id: i64,
}
