use crate::repositories::ethers::anvil_repository::{AnvilRepository, ManagedAnvilInstance};
use ethers::prelude::Provider;
use ethers::providers::{Http, Middleware};
use ethers::types::{Bytes, TransactionRequest, U256, H160};
use ethers::utils::hex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};

/// Legacy inspector contract bytecode (performs buy+sell on fork, measures gas via gasleft())
const INSPECTOR_BYTECODE: &str = "6080604052737a250d5630b4cf539739df2c5dacb4c659f2488d600160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff16021790555034801561006557600080fd5b5061008261007761014b60201b60201c565b61015360201b60201c565b6001600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548160ff02191690831515021790555060016002600073f034ed66c467cb1ef9a48964d77a377d781ca78673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548160ff021916908315150217905550610217565b600033905090565b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff169050816000806101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff1602179055508173ffffffffffffffffffffffffffffffffffffffff168173ffffffffffffffffffffffffffffffffffffffff167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e060405160405180910390a35050565b611efd806102266000396000f3fe6080604052600436106100c65760003560e01c80638da5cb5b1161007f578063c0d7865511610059578063c0d7865514610416578063caf5f67d1461043f578063f2fde38b14610456578063f887ea401461047f5761026b565b80638da5cb5b14610397578063a1764595146103c2578063af04d061146103ed5761026b565b806304f4b9c51461027057806305d162401461029957806324d7806c146102db5780632e8602c814610318578063715018a614610355578063722713f71461036c5761026b565b3661026b573373ffffffffffffffffffffffffffffffffffffffff166100ea6104aa565b73ffffffffffffffffffffffffffffffffffffffff160361026957600061010f6104d3565b9050600061013f82600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1661056b565b03610206578073ffffffffffffffffffffffffffffffffffffffff1663095ea7b3600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff167fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff6040518363ffffffff1660e01b81526004016101c19291906114c8565b6020604051808303816000875af11580156101e0573d6000803e3d6000fd5b505050506040513d601f19601f82011682018060405250810190610204919061153d565b505b8073ffffffffffffffffffffffffffffffffffffffff1663d0e30db0346040518263ffffffff1660e01b81526004016000604051808303818588803b15801561024e57600080fd5b505af1158015610262573d6000803e3d6000fd5b5050505050505b005b600080fd5b34801561027c57600080fd5b50610297600480360381019061029291906116ef565b6105f1565b005b3480156102a557600080fd5b506102c060048036038101906102bb9190611764565b610694565b6040516102d2969594939291906117b3565b60405180910390f35b3480156102e757600080fd5b5061030260048036038101906102fd9190611814565b6109b3565b60405161030f9190611850565b60405180910390f35b34801561032457600080fd5b5061033f600480360381019061033a919061186b565b610a09565b60405161034c9190611985565b60405180910390f35b34801561036157600080fd5b5061036a610ab6565b005b34801561037857600080fd5b50610381610aca565b60405161038e91906119a7565b60405180910390f35b3480156103a357600080fd5b506103ac6104aa565b6040516103b991906119c2565b60405180910390f35b3480156103ce57600080fd5b506103d76104d3565b6040516103e491906119c2565b60405180910390f35b3480156103f957600080fd5b50610414600480360381019061040f9190611814565b610b5a565b005b34801561042257600080fd5b5061043d60048036038101906104389190611814565b610bc4565b005b34801561044b57600080fd5b50610454610c10565b005b34801561046257600080fd5b5061047d60048036038101906104789190611814565b610d58565b005b34801561048b57600080fd5b50610494610ddb565b6040516104a191906119c2565b60405180910390f35b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff16905090565b6000600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1663ad5c46486040518163ffffffff1660e01b8152600401602060405180830381865afa158015610542573d6000803e3d6000fd5b505050506040513d601f19601f8201168201806040525081019061056691906119f2565b905090565b60008273ffffffffffffffffffffffffffffffffffffffff1663dd62ed3e30846040518363ffffffff1660e01b81526004016105a8929190611a1f565b602060405180830381865afa1580156105c5573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906105e99190611a5d565b905092915050565b6105f9610e01565b60008151905060005b8181101561068f5760016002600085848151811061062357610622611a8a565b5b602002602001015173ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548160ff021916908315150217905550808061068790611ae8565b915050610602565b505050565b600080600080600080600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff166106f357600080fd5b60006106fe89610e7f565b9050600061070b8a610f7d565b90508060008151811061072157610720611a8a565b5b602002602001015173ffffffffffffffffffffffffffffffffffffffff1663095ea7b3600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff167fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff6040518363ffffffff1660e01b81526004016107a59291906114c8565b6020604051808303816000875af11580156107c4573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906107e8919061153d565b50806001815181106107fd576107fc611a8a565b5b602002602001015173ffffffffffffffffffffffffffffffffffffffff1663095ea7b3600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff167fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff6040518363ffffffff1660e01b81526004016108819291906114c8565b6020604051808303816000875af11580156108a0573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906108c4919061153d565b50806000815181106108d9576108d8611a8a565b5b602002602001015173ffffffffffffffffffffffffffffffffffffffff1663d0e30db08a6040518263ffffffff1660e01b81526004016000604051808303818588803b15801561092857600080fd5b505af115801561093c573d6000803e3d6000fd5b505050505060005a9050600080610953848d61107b565b9150915060005a846109659190611b30565b905060005a9050600080610979898761107b565b9150915060005a8461098b9190611b30565b90508686848488859f509f509f509f509f509f50505050505050505050509295509295509295565b6000600260008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff169050919050565b6060600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1663d06ca61f83856040518363ffffffff1660e01b8152600401610a68929190611c22565b600060405180830381865afa158015610a85573d6000803e3d6000fd5b505050506040513d6000823e3d601f19601f82011682018060405250810190610aae9190611d15565b905092915050565b610abe610e01565b610ac8600061124f565b565b6000610ad4610e01565b610adc6104d3565b73ffffffffffffffffffffffffffffffffffffffff166370a08231306040518263ffffffff1660e01b8152600401610b1491906119c2565b602060405180830381865afa158015610b31573d6000803e3d6000fd5b505050506040513d601f19601f82011682018060405250810190610b559190611a5d565b905090565b600360008273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600043815260200190815260200160002060009054906101000a900460ff16610bc157600080fd5b50565b610bcc610e01565b80600160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff16021790555050565b610c18610e01565b6000610c226104d3565b905060008173ffffffffffffffffffffffffffffffffffffffff166370a08231306040518263ffffffff1660e01b8152600401610c5f91906119c2565b602060405180830381865afa158015610c7c573d6000803e3d6000fd5b505050506040513d601f19601f82011682018060405250810190610ca09190611a5d565b90508173ffffffffffffffffffffffffffffffffffffffff16632e1a7d4d826040518263ffffffff1660e01b8152600401610cdb91906119a7565b600060405180830381600087803b158015610cf557600080fd5b505af1158015610d09573d6000803e3d6000fd5b505050503373ffffffffffffffffffffffffffffffffffffffff166108fc829081150290604051600060405180830381858888f19350505050158015610d53573d6000803e3d6000fd5b505050565b610d60610e01565b600073ffffffffffffffffffffffffffffffffffffffff168173ffffffffffffffffffffffffffffffffffffffff1603610dcf576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610dc690611de1565b60405180910390fd5b610dd88161124f565b50565b600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1681565b610e09611313565b73ffffffffffffffffffffffffffffffffffffffff16610e276104aa565b73ffffffffffffffffffffffffffffffffffffffff1614610e7d576040517f08c379a0000000000000000000000000000000000000000000000000000000008152600401610e7490611e4d565b60405180910390fd5b565b60606000600267ffffffffffffffff811115610e9e57610e9d611580565b5b604051908082528060200260200182016040528015610ecc5781602001602082028036833780820191505090505b5090508281600081518110610ee457610ee3611a8a565b5b602002602001019073ffffffffffffffffffffffffffffffffffffffff16908173ffffffffffffffffffffffffffffffffffffffff1681525050610f266104d3565b81600181518110610f3a57610f39611a8a565b5b602002602001019073ffffffffffffffffffffffffffffffffffffffff16908173ffffffffffffffffffffffffffffffffffffffff168152505080915050919050565b60606000600267ffffffffffffffff811115610f9c57610f9b611580565b5b604051908082528060200260200182016040528015610fca5781602001602082028036833780820191505090505b509050610fd56104d3565b81600081518110610fe957610fe8611a8a565b5b602002602001019073ffffffffffffffffffffffffffffffffffffffff16908173ffffffffffffffffffffffffffffffffffffffff1681525050828160018151811061103857611037611a8a565b5b602002602001019073ffffffffffffffffffffffffffffffffffffffff16908173ffffffffffffffffffffffffffffffffffffffff168152505080915050919050565b60008060006110c78560018151811061109757611096611a8a565b5b6020026020010151600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1661056b565b036111a957836001815181106110e0576110df611a8a565b5b602002602001015173ffffffffffffffffffffffffffffffffffffffff1663095ea7b3600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff167fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff6040518363ffffffff1660e01b81526004016111649291906114c8565b6020604051808303816000875af1158015611183573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906111a7919061153d565b505b60006111b58585610a09565b905060006111dd866001815181106111d0576111cf611a8a565b5b602002602001015161131b565b90506111eb8686600061139e565b60006112118760018151811061120457611203611a8a565b5b602002602001015161131b565b9050600082826112219190611b30565b9050808460018151811061123857611237611a8a565b5b602002602001015195509550505050509250929050565b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff169050816000806101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff1602179055508173ffffffffffffffffffffffffffffffffffffffff168173ffffffffffffffffffffffffffffffffffffffff167f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e060405160405180910390a35050565b600033905090565b60008173ffffffffffffffffffffffffffffffffffffffff166370a08231306040518263ffffffff1660e01b815260040161135691906119c2565b602060405180830381865afa158015611373573d6000803e3d6000fd5b505050506040513d601f19601f820116820180604052508101906113979190611a5d565b9050919050565b600160009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16635c11d79583838630426040518663ffffffff1660e01b8152600401611401959493929190611e6d565b600060405180830381600087803b15801561141b57600080fd5b505af115801561142f573d6000803e3d6000fd5b50505050505050565b600073ffffffffffffffffffffffffffffffffffffffff82169050919050565b600061146382611438565b9050919050565b61147381611458565b82525050565b6000819050919050565b6000819050919050565b6000819050919050565b60006114b26114ad6114a884611479565b61148d565b611483565b9050919050565b6114c281611497565b82525050565b60006040820190506114dd600083018561146a565b6114ea60208301846114b9565b9392505050565b6000604051905090565b600080fd5b600080fd5b60008115159050919050565b61151a81611505565b811461152557600080fd5b50565b60008151905061153781611511565b92915050565b600060208284031215611553576115526114fb565b5b600061156184828501611528565b91505092915050565b600080fd5b6000601f19601f8301169050919050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052604160045260246000fd5b6115b88261156f565b810181811067ffffffffffffffff821117156115d7576115d6611580565b5b80604052505050565b60006115ea6114f1565b90506115f682826115af565b919050565b600067ffffffffffffffff82111561161657611615611580565b5b602082029050602081019050919050565b600080fd5b61163581611458565b811461164057600080fd5b50565b6000813590506116528161162c565b92915050565b600061166b611666846115fb565b6115e0565b9050808382526020820190506020840283018581111561168e5761168d611627565b5b835b818110156116b757806116a38882611643565b845260208401935050602081019050611690565b5050509392505050565b600082601f8301126116d6576116d561156a565b5b81356116e6848260208601611658565b91505092915050565b600060208284031215611705576117046114fb565b5b600082013567ffffffffffffffff81111561172357611722611500565b5b61172f848285016116c1565b91505092915050565b61174181611483565b811461174c57600080fd5b50565b60008135905061175e81611738565b92915050565b6000806040838503121561177b5761177a6114fb565b5b600061178985828601611643565b925050602061179a8582860161174f565b9150509250929050565b6117ad81611483565b82525050565b600060c0820190506117c860008301896117a4565b6117d560208301886117a4565b6117e260408301876117a4565b6117ef60608301866117a4565b6117fc60808301856117a4565b61180960a08301846117a4565b979650505050505050565b60006020828403121561182a576118296114fb565b5b600061183884828501611643565b91505092915050565b61184a81611505565b82525050565b60006020820190506118656000830184611841565b92915050565b60008060408385031215611882576118816114fb565b5b600083013567ffffffffffffffff8111156118a05761189f611500565b5b6118ac858286016116c1565b92505060206118bd8582860161174f565b9150509250929050565b600081519050919050565b600082825260208201905092915050565b6000819050602082019050919050565b6118fc81611483565b82525050565b600061190e83836118f3565b60208301905092915050565b6000602082019050919050565b6000611932826118c7565b61193c81856118d2565b9350611947836118e3565b8060005b8381101561197857815161195f8882611902565b975061196a8361191a565b92505060018101905061194b565b5085935050505092915050565b6000602082019050818103600083015261199f8184611927565b905092915050565b60006020820190506119bc60008301846117a4565b92915050565b60006020820190506119d7600083018461146a565b92915050565b6000815190506119ec8161162c565b92915050565b600060208284031215611a0857611a076114fb565b5b6000611a16848285016119dd565b91505092915050565b6000604082019050611a34600083018561146a565b611a41602083018461146a565b9392505050565b600081519050611a5781611738565b92915050565b600060208284031215611a7357611a726114fb565b5b6000611a8184828501611a48565b91505092915050565b7f4e487b7100000000000000000000000000000000000000000000000000000000600052603260045260246000fd5b7f4e487b7100000000000000000000000000000000000000000000000000000000600052601160045260246000fd5b6000611af382611483565b91507fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff8203611b2557611b24611ab9565b5b600182019050919050565b6000611b3b82611483565b9150611b4683611483565b9250828203905081811115611b5e57611b5d611ab9565b5b92915050565b600081519050919050565b600082825260208201905092915050565b6000819050602082019050919050565b611b9981611458565b82525050565b6000611bab8383611b90565b60208301905092915050565b6000602082019050919050565b6000611bcf82611b64565b611bd98185611b6f565b9350611be483611b80565b8060005b83811015611c15578151611bfc8882611b9f565b9750611c0783611bb7565b925050600181019050611be8565b5085935050505092915050565b6000604082019050611c3760008301856117a4565b8181036020830152611c498184611bc4565b90509392505050565b600067ffffffffffffffff821115611c6d57611c6c611580565b5b602082029050602081019050919050565b6000611c91611c8c84611c52565b6115e0565b90508083825260208201905060208402830185811115611cb457611cb3611627565b5b835b81811015611cdd5780611cc98882611a48565b845260208401935050602081019050611cb6565b5050509392505050565b600082601f830112611cfc57611cfb61156a565b5b8151611d0c848260208601611c7e565b91505092915050565b600060208284031215611d2b57611d2a6114fb565b5b600082015167ffffffffffffffff811115611d4957611d48611500565b5b611d5584828501611ce7565b91505092915050565b600082825260208201905092915050565b7f4f776e61626c653a206e6577206f776e657220697320746865207a65726f206160008201527f6464726573730000000000000000000000000000000000000000000000000000602082015250565b6000611dcb602683611d5e565b9150611dd682611d6f565b604082019050919050565b60006020820190508181036000830152611dfa81611dbe565b9050919050565b7f4f776e61626c653a2063616c6c6572206973206e6f7420746865206f776e6572600082015250565b6000611e37602083611d5e565b9150611e4282611e01565b602082019050919050565b60006020820190508181036000830152611e6681611e2a565b9050919050565b600060a082019050611e8260008301886117a4565b611e8f60208301876117a4565b8181036040830152611ea18186611bc4565b9050611eb0606083018561146a565b611ebd60808301846117a4565b969550505050505056fea26469706673582212202ecfcd348ccaae833bd0e93aec0cc071363cc06588f10981b2641c7ac3883e8b64736f6c63430008130033";

pub struct AnvilSimulation {
    repository: Arc<RwLock<AnvilRepository>>,
    concurrency: Arc<Semaphore>,
    timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub buy_gas: u64,
    pub sell_gas: u64,
    pub buy_tax: f64,
    pub sell_tax: f64,
    pub max_buy: Option<U256>,
    pub is_scam: bool,
}

impl AnvilSimulation {
    pub fn new(repository: Arc<RwLock<AnvilRepository>>) -> Self {
        let concurrency = std::env::var("ANVIL_SIM_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(2);
        let timeout_secs = std::env::var("ANVIL_SIM_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v >= 10)
            .unwrap_or(75);

        AnvilSimulation {
            repository,
            concurrency: Arc::new(Semaphore::new(concurrency)),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub async fn simulate(
        &self,
        rpc_endpoint: &str,
        token_address: H160,
        deployer: H160,
        bytecode: &str,
        block_number: Option<u64>,
    ) -> Option<SimulationResult> {
        let identifier = format!(
            "sim_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        eprintln!("[ANVIL] Iniciando simulacao para {:?}", token_address);

        let _permit = match self.concurrency.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                eprintln!("[ANVIL] Semaforo de simulacao fechado");
                return None;
            }
        };

        let result = match tokio::time::timeout(
            self.timeout,
            self.simulate_inner(
                rpc_endpoint,
                &identifier,
                token_address,
                deployer,
                bytecode,
                block_number,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                eprintln!(
                    "[ANVIL] Timeout de {:?} na simulacao de {:?}",
                    self.timeout, token_address
                );
                None
            }
        };

        self.cleanup(&identifier).await;
        eprintln!("[ANVIL] Resultado: {:?}", result);
        result
    }

    async fn simulate_inner(
        &self,
        rpc_endpoint: &str,
        identifier: &str,
        token_address: H160,
        deployer: H160,
        bytecode: &str,
        block_number: Option<u64>,
    ) -> Option<SimulationResult> {
        if let Err(e) = self.create_fork(rpc_endpoint, identifier, block_number).await {
            eprintln!("[ANVIL] Erro ao criar fork: {}", e);
            return None;
        }

        if self.set_balance(identifier, deployer).await.is_err() {
            return None;
        }

        self.run_simulation(identifier, token_address, deployer, bytecode).await
    }

    async fn create_fork(&self, endpoint: &str, identifier: &str, block_number: Option<u64>) -> Result<(), String> {
        let (provider, anvil) =
            ManagedAnvilInstance::spawn_forking_provider(endpoint, block_number).await?;

        let block_id = ethers::types::BlockId::Number(
            ethers::types::BlockNumber::Number(block_number.unwrap_or(0).into())
        );
        let timestamp = match provider.get_block(block_id).await {
            Ok(Some(block)) => block.timestamp.as_u64(),
            _ => 0,
        };

        let mut repo = self.repository.write().await;
        repo.set_block_timestamp(identifier.to_string(), timestamp);
        repo.apply_forking_provider(identifier.to_string(), provider, anvil);

        Ok(())
    }

    async fn set_balance(&self, identifier: &str, address: H160) -> Result<(), String> {
        let balance = ethers::utils::parse_ether(9999).map_err(|e| e.to_string())?;
        let repo = self.repository.read().await;
        repo.set_balance(identifier.to_string(), address, balance).await;
        Ok(())
    }

    async fn run_simulation(
        &self,
        identifier: &str,
        token_address: H160,
        deployer: H160,
        bytecode: &str,
    ) -> Option<SimulationResult> {
        let buyer: H160 = "000000000000000000000000000000000000dEaD".parse().ok()?;

        // Set balances: deployer, contract, and buyer all need ETH
        {
            let repo = self.repository.read().await;
            let balance = ethers::utils::parse_ether(100).ok()?;
            repo.set_balance(identifier.to_string(), buyer, balance).await;
            // Contract needs ETH for openTrading -> addLiquidityETH
            let contract_balance = ethers::utils::parse_ether(10).ok()?;
            repo.set_balance(identifier.to_string(), token_address, contract_balance).await;
        }

        // Try ALL matching openTrading signatures
        {
            let repo = self.repository.read().await;
            let provider = repo.get_fork_connection(identifier.to_string())?;

            let open_trading_sigs = [
                "c9567bf9", "8f70ccf7", "8a8c523c",
                "293230b8", "bccb3916", "0e4ce20f",
                "fe575a87", "04eb5337",
            ];

            let mut any_matched = false;
            for sig in &open_trading_sigs {
                if bytecode.contains(sig) {
                    let data = hex::decode(sig).unwrap_or_default();
                    let tx = TransactionRequest::new()
                        .from(deployer)
                        .to(token_address)
                        .data(Bytes::from(data));
                    match provider.send_transaction(tx, None).await {
                        Ok(pending) => {
                            eprintln!("[ANVIL] openTrading sig {} sent OK: {:?}", sig, pending.tx_hash());
                        }
                        Err(e) => {
                            eprintln!("[ANVIL] openTrading sig {} FAILED: {}", sig, e);
                        }
                    }
                    any_matched = true;
                }
            }

            // Also try sending ETH from deployer to contract (some contracts need this)
            if any_matched {
                let send_eth = TransactionRequest::new()
                    .from(deployer)
                    .to(token_address)
                    .value(ethers::utils::parse_ether(5).ok()?);
                let _ = provider.send_transaction(send_eth, None).await;
            }

            drop(provider);
            drop(repo);

            if any_matched {
                let repo = self.repository.read().await;
                repo.mine(identifier.to_string()).await;
            }
        }

        // Check if pair was created on the fork
        let pair_exists = {
            let repo = self.repository.read().await;
            let mut has_pair = false;
            if let Some(provider) = repo.get_fork_connection(identifier.to_string()) {
                let factory: H160 = "5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f".parse().unwrap();
                let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().unwrap();
                let mut calldata = hex::decode("e6a43905").unwrap_or_default();
                calldata.extend_from_slice(&[0u8; 12]);
                calldata.extend_from_slice(token_address.as_bytes());
                calldata.extend_from_slice(&[0u8; 12]);
                calldata.extend_from_slice(weth.as_bytes());
                let tx = TransactionRequest::new().to(factory).data(Bytes::from(calldata));
                let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();
                match provider.call(&typed, None).await {
                    Ok(result) if result.len() >= 32 => {
                        let pair = H160::from_slice(&result[12..32]);
                        has_pair = pair != H160::zero();
                        eprintln!("[ANVIL] Fork pair check: {:?} (zero={})", pair, pair == H160::zero());
                    }
                    Ok(_) => eprintln!("[ANVIL] Fork pair check: empty result"),
                    Err(e) => eprintln!("[ANVIL] Fork pair check error: {}", e),
                }
            }
            has_pair
        };

        // If no pair exists, try manual addLiquidityETH (like Legacy bot)
        if !pair_exists {
            eprintln!("[ANVIL] No pair found, trying manual addLiquidityETH...");
            self.try_add_liquidity(identifier, token_address, deployer).await;
        }

        let max_buy = self.try_max_buy(identifier, token_address).await;

        // Match Legacy seekers-galaxy: rely SOLELY on the inspector contract
        // (deploy → setRouter → inspect(token, 0.01 ETH)) for both gas and
        // tax measurement. Legacy intentionally does NOT read storage-level
        // _buyFee()/_sellFee() selectors, because templates with `reduceFee`
        // expose post-reduction values that do not reflect the tax actually
        // charged on the *first* swap. The inspector observes the real
        // expected-vs-received delta on a buy+sell pair, which is what the
        // operator's mental model is anchored to.
        let inspect_result = self.run_inspector(identifier, token_address, deployer).await;

        let (buy_gas, sell_gas, buy_tax, sell_tax) = match inspect_result {
            Some((bg, sg, bt, st)) => (bg, sg, bt, st),
            None => {
                eprintln!("[ANVIL] Inspector failed, falling back to estimate_gas");
                let bg = self.simulate_buy(identifier, token_address, buyer).await.unwrap_or(0);
                if bg > 0 {
                    self.execute_buy(identifier, token_address, buyer).await;
                    let repo = self.repository.read().await;
                    repo.mine(identifier.to_string()).await;
                }
                let sg = self.simulate_sell(identifier, token_address, buyer).await.unwrap_or(0);
                (bg, sg, 0.0, 0.0)
            }
        };

        Some(SimulationResult {
            buy_gas,
            sell_gas,
            buy_tax,
            sell_tax,
            max_buy,
            is_scam: false,
        })
    }

    /// Legacy inspector contract: deploys on fork, does buy+sell internally, returns gas via gasleft()
    async fn run_inspector(
        &self,
        identifier: &str,
        token_address: H160,
        deployer: H160,
    ) -> Option<(u64, u64, f64, f64)> {
        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        // Compute next contract address for the inspector (CREATE with deployer nonce)
        let nonce = provider.get_transaction_count(deployer, None).await.ok()?;
        let inspector_addr = ethers::utils::get_contract_address(deployer, nonce);
        eprintln!("[ANVIL] Inspector: deploying at {:?} (nonce={})", inspector_addr, nonce);

        // Deploy inspector contract (send_transaction, then mine)
        let deploy_tx = TransactionRequest::new()
            .from(deployer)
            .data(Bytes::from(hex::decode(INSPECTOR_BYTECODE).ok()?));
        match provider.send_transaction(deploy_tx, None).await {
            Ok(_) => {},
            Err(e) => {
                eprintln!("[ANVIL] Inspector deploy failed: {}", e);
                return None;
            }
        }

        drop(provider);
        drop(repo);
        {
            let repo = self.repository.read().await;
            repo.mine(identifier.to_string()).await;
        }

        // Set balance for inspector (it needs ETH to wrap into WETH)
        {
            let repo = self.repository.read().await;
            let balance = ethers::utils::parse_ether(100).ok()?;
            repo.set_balance(identifier.to_string(), inspector_addr, balance).await;
        }

        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        // setRouter(address) on inspector - selector 0xc0d78655
        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;
        let mut set_router_data = hex::decode("c0d78655").ok()?;
        set_router_data.extend_from_slice(&[0u8; 12]);
        set_router_data.extend_from_slice(router.as_bytes());

        let set_router_tx = TransactionRequest::new()
            .from(deployer)
            .to(inspector_addr)
            .data(Bytes::from(set_router_data));
        let _ = provider.send_transaction(set_router_tx, None).await;

        drop(provider);
        drop(repo);
        {
            let repo = self.repository.read().await;
            repo.mine(identifier.to_string()).await;
        }

        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        // inspect(address tokenAddress, uint256 ethAmount) - selector 0x05d16240
        let amount = ethers::utils::parse_ether("0.01").ok()?;
        let mut inspect_data = hex::decode("05d16240").ok()?;
        inspect_data.extend_from_slice(&[0u8; 12]);
        inspect_data.extend_from_slice(token_address.as_bytes());
        inspect_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(amount)]));

        let inspect_tx = TransactionRequest::new()
            .from(deployer)
            .to(inspector_addr)
            .data(Bytes::from(inspect_data));
        let typed: ethers::types::transaction::eip2718::TypedTransaction = inspect_tx.into();

        let output = match provider.call(&typed, None).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[ANVIL] Inspector call failed: {}", e);
                return None;
            }
        };

        // Decode: (uint256 brcv, uint256 bout, uint256 srcv, uint256 sout, uint256 buyGas, uint256 sellGas)
        if output.len() < 192 {
            eprintln!("[ANVIL] Inspector output too short: {} bytes", output.len());
            return None;
        }

        // Inspector ABI return tuple (mirrors Legacy seekers-galaxy decode):
        //   (brcv, bout, srcv, sout, buyGas, sellGas)
        //
        //   brcv = ACTUAL tokens received from the buy swap (post-tax)
        //   bout = EXPECTED tokens from getAmountsOut (pre-tax quote)
        //   srcv = ACTUAL ETH received from the sell swap (post-tax)
        //   sout = EXPECTED ETH from getAmountsOut for the sell (pre-tax quote)
        //
        // Tax convention (matches Legacy bot output 0–100%):
        //   buy_tax  = (bout - brcv) / bout * 100     (i.e. % of expected output lost to tax)
        //   sell_tax = (sout - srcv) / sout * 100
        // Special case: if brcv == 0 the buy itself failed → sell impossible → 100%.
        let brcv = U256::from_big_endian(&output[0..32]);
        let bout = U256::from_big_endian(&output[32..64]);
        let srcv = U256::from_big_endian(&output[64..96]);
        let sout = U256::from_big_endian(&output[96..128]);
        let buy_gas_u = U256::from_big_endian(&output[128..160]);
        let sell_gas_u = U256::from_big_endian(&output[160..192]);

        let buy_gas = if buy_gas_u > U256::from(u64::MAX) { u64::MAX } else { buy_gas_u.as_u64() };
        let sell_gas = if sell_gas_u > U256::from(u64::MAX) { u64::MAX } else { sell_gas_u.as_u64() };

        let pct_drop = |actual: U256, expected: U256| -> f64 {
            if expected.is_zero() {
                return 0.0;
            }
            if actual.is_zero() {
                return 100.0;
            }
            if actual >= expected {
                return 0.0;
            }
            let diff = expected - actual;
            let d = if diff > U256::from(u128::MAX) { u128::MAX } else { diff.as_u128() };
            let e = if expected > U256::from(u128::MAX) { u128::MAX } else { expected.as_u128() };
            (d as f64 / e as f64) * 100.0
        };

        let buy_tax = pct_drop(brcv, bout);
        let sell_tax = if brcv.is_zero() {
            100.0
        } else {
            pct_drop(srcv, sout)
        };

        eprintln!(
            "[ANVIL] Inspector result: brcv={} bout={} srcv={} sout={} buy_gas={} sell_gas={} buy_tax={:.2} sell_tax={:.2}",
            brcv, bout, srcv, sout, buy_gas, sell_gas, buy_tax, sell_tax,
        );
        Some((buy_gas, sell_gas, buy_tax, sell_tax))
    }

    async fn simulate_buy(
        &self,
        identifier: &str,
        token: H160,
        buyer: H160,
    ) -> Option<u64> {
        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;
        let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().ok()?;

        let amount_in = ethers::utils::parse_ether("0.01").ok()?;

        let deadline = U256::from(u64::MAX);
        // swapExactETHForTokens (standard, matches Legacy gas values)
        let mut calldata = hex::decode("7ff36ab5").ok()?;
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::zero())]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(128u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(buyer.as_bytes());
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(deadline)]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(2u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());

        let tx = TransactionRequest::new()
            .from(buyer)
            .to(router)
            .data(Bytes::from(calldata.clone()))
            .value(amount_in);
        let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();

        match provider.estimate_gas(&typed, None).await {
            Ok(g) => Some(if g > U256::from(u64::MAX) { u64::MAX } else { g.as_u64() }),
            Err(_) => {
                // Fallback: try SupportingFeeOnTransferTokens if standard fails
                let mut fb_calldata = hex::decode("b6f9de95").ok()?;
                fb_calldata.extend_from_slice(&calldata[4..]);
                let fb_tx = TransactionRequest::new()
                    .from(buyer)
                    .to(router)
                    .data(Bytes::from(fb_calldata))
                    .value(amount_in);
                let fb_typed: ethers::types::transaction::eip2718::TypedTransaction = fb_tx.into();
                match provider.estimate_gas(&fb_typed, None).await {
                    Ok(g) => Some(if g > U256::from(u64::MAX) { u64::MAX } else { g.as_u64() }),
                    Err(_) => None,
                }
            }
        }
    }

    async fn execute_buy(
        &self,
        identifier: &str,
        token: H160,
        buyer: H160,
    ) {
        let repo = self.repository.read().await;
        let provider = match repo.get_fork_connection(identifier.to_string()) {
            Some(p) => p,
            None => return,
        };

        let router: H160 = match "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse() {
            Ok(r) => r,
            Err(_) => return,
        };
        let weth: H160 = match "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse() {
            Ok(w) => w,
            Err(_) => return,
        };

        let amount_in = match ethers::utils::parse_ether("0.01") {
            Ok(a) => a,
            Err(_) => return,
        };
        let deadline = U256::from(u64::MAX);

        // swapExactETHForTokensSupportingFeeOnTransferTokens
        let mut calldata = match hex::decode("b6f9de95") {
            Ok(d) => d,
            Err(_) => return,
        };
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::zero())]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(128u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(buyer.as_bytes());
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(deadline)]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(2u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());

        let tx = TransactionRequest::new()
            .from(buyer)
            .to(router)
            .data(Bytes::from(calldata))
            .value(amount_in);
        let _ = provider.send_transaction(tx, None).await;
    }

    async fn simulate_sell(
        &self,
        identifier: &str,
        token: H160,
        seller: H160,
    ) -> Option<u64> {
        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;
        let weth: H160 = "C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".parse().ok()?;

        // Get seller's token balance
        let token_balance = self.get_token_balance(&provider, token, seller).await?;
        eprintln!("[ANVIL] Sell: seller {:?} token balance = {}", seller, token_balance);
        if token_balance == U256::zero() {
            eprintln!("[ANVIL] Sell: no tokens to sell, buyer may not have received tokens from buy");
            return Some(0);
        }

        let sell_amount = token_balance / U256::from(2);
        if sell_amount == U256::zero() {
            return Some(0);
        }

        // Approve router
        let mut approve_data = hex::decode("095ea7b3").ok()?;
        approve_data.extend_from_slice(&[0u8; 12]);
        approve_data.extend_from_slice(router.as_bytes());
        approve_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::MAX)]));

        let approve_tx = TransactionRequest::new()
            .from(seller)
            .to(token)
            .data(Bytes::from(approve_data));
        let _ = provider.send_transaction(approve_tx, None).await;

        {
            drop(provider);
            drop(repo);
            let repo2 = self.repository.read().await;
            repo2.mine(identifier.to_string()).await;
        }

        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        let deadline = U256::from(u64::MAX);
        // swapExactTokensForETH (standard, matches Legacy gas values)
        let mut calldata = hex::decode("18cbafe5").ok()?;
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(sell_amount)]));
        calldata.extend_from_slice(&[0u8; 32]); // amountOutMin = 0
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(160u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(seller.as_bytes());
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(deadline)]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(2u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(weth.as_bytes());

        let tx = TransactionRequest::new()
            .from(seller)
            .to(router)
            .data(Bytes::from(calldata.clone()));
        let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();

        match provider.estimate_gas(&typed, None).await {
            Ok(g) => Some(if g > U256::from(u64::MAX) { u64::MAX } else { g.as_u64() }),
            Err(_) => {
                // Fallback: try SupportingFeeOnTransferTokens if standard fails
                let mut fb_calldata = hex::decode("791ac947").ok()?;
                fb_calldata.extend_from_slice(&calldata[4..]);
                let fb_tx = TransactionRequest::new()
                    .from(seller)
                    .to(router)
                    .data(Bytes::from(fb_calldata));
                let fb_typed: ethers::types::transaction::eip2718::TypedTransaction = fb_tx.into();
                match provider.estimate_gas(&fb_typed, None).await {
                    Ok(g) => Some(if g > U256::from(u64::MAX) { u64::MAX } else { g.as_u64() }),
                    Err(e) => {
                        eprintln!("[ANVIL] sell gas estimate failed on fork: {}", e);
                        None
                    }
                }
            }
        }
    }

    async fn get_amounts_out(
        &self,
        provider: &Arc<Provider<Http>>,
        amount_in: U256,
        token_in: &H160,
        token_out: &H160,
    ) -> Option<U256> {
        let router: H160 = "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse().ok()?;

        // getAmountsOut(uint256,address[])
        let mut calldata = hex::decode("d06ca61f").ok()?;
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(amount_in)]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(64u64))]));
        calldata.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::from(2u64))]));
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token_in.as_bytes());
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(token_out.as_bytes());

        let tx = TransactionRequest::new()
            .to(router)
            .data(Bytes::from(calldata));
        let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();

        let result = provider.call(&typed, None).await.ok()?;
        if result.len() >= 96 {
            Some(U256::from_big_endian(&result[64..96]))
        } else {
            None
        }
    }

    async fn get_token_balance(
        &self,
        provider: &Arc<Provider<Http>>,
        token: H160,
        account: H160,
    ) -> Option<U256> {
        let mut calldata = hex::decode("70a08231").ok()?;
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(account.as_bytes());

        let tx = TransactionRequest::new()
            .to(token)
            .data(Bytes::from(calldata));
        let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();

        let result = provider.call(&typed, None).await.ok()?;
        if result.len() >= 32 {
            Some(U256::from_big_endian(&result[..32]))
        } else {
            None
        }
    }

    async fn try_max_buy(&self, identifier: &str, token: H160) -> Option<U256> {
        let repo = self.repository.read().await;
        let provider = repo.get_fork_connection(identifier.to_string())?;

        let function_selectors = [
            "8f9a55c0", "e8078d94", "3582ad23", "cf188ad0",
        ];

        for sel in &function_selectors {
            let data = hex::decode(sel).ok()?;
            let tx = TransactionRequest::new()
                .to(token)
                .data(Bytes::from(data));
            let typed: ethers::types::transaction::eip2718::TypedTransaction = tx.into();

            if let Ok(result) = provider.call(&typed, None).await {
                if result.len() >= 32 {
                    let val = U256::from_big_endian(&result[..32]);
                    if val > U256::zero() {
                        return Some(val);
                    }
                }
            }
        }

        None
    }

    async fn try_add_liquidity(
        &self,
        identifier: &str,
        token: H160,
        deployer: H160,
    ) {
        let router: H160 = match "7a250d5630B4cF539739dF2C5dAcb4c659F2488D".parse() {
            Ok(r) => r,
            Err(_) => return,
        };

        let repo = self.repository.read().await;
        let provider = match repo.get_fork_connection(identifier.to_string()) {
            Some(p) => p,
            None => return,
        };

        // 1. Get deployer's token balance
        let mut bal_data = match hex::decode("70a08231") {
            Ok(d) => d,
            Err(_) => return,
        };
        bal_data.extend_from_slice(&[0u8; 12]);
        bal_data.extend_from_slice(deployer.as_bytes());
        let bal_tx = TransactionRequest::new().to(token).data(Bytes::from(bal_data));
        let typed_bal: ethers::types::transaction::eip2718::TypedTransaction = bal_tx.into();
        let token_balance = match provider.call(&typed_bal, None).await {
            Ok(result) if result.len() >= 32 => U256::from_big_endian(&result[..32]),
            _ => return,
        };

        if token_balance == U256::zero() {
            eprintln!("[ANVIL] addLiquidity: deployer has 0 tokens");
            return;
        }
        eprintln!("[ANVIL] addLiquidity: deployer token balance = {}", token_balance);

        // 2. Approve router for MAX
        let mut approve_data = match hex::decode("095ea7b3") {
            Ok(d) => d,
            Err(_) => return,
        };
        approve_data.extend_from_slice(&[0u8; 12]);
        approve_data.extend_from_slice(router.as_bytes());
        approve_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::MAX)]));
        let approve_tx = TransactionRequest::new()
            .from(deployer)
            .to(token)
            .data(Bytes::from(approve_data));
        let _ = provider.send_transaction(approve_tx, None).await;

        // 3. addLiquidityETH(token, amountTokenDesired, 0, 0, deployer, deadline)
        // Use 90% of deployer's token balance for liquidity
        let token_amount = token_balance * U256::from(90u64) / U256::from(100u64);
        let eth_amount = match ethers::utils::parse_ether(1) {
            Ok(a) => a,
            Err(_) => return,
        };
        let deadline = U256::from(u64::MAX);

        // addLiquidityETH selector = f305d719
        let mut liq_data = match hex::decode("f305d719") {
            Ok(d) => d,
            Err(_) => return,
        };
        liq_data.extend_from_slice(&[0u8; 12]);
        liq_data.extend_from_slice(token.as_bytes());
        liq_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(token_amount)]));
        liq_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::zero())]));
        liq_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(U256::zero())]));
        liq_data.extend_from_slice(&[0u8; 12]);
        liq_data.extend_from_slice(deployer.as_bytes());
        liq_data.extend_from_slice(&ethers::abi::encode(&[ethers::abi::Token::Uint(deadline)]));

        let liq_tx = TransactionRequest::new()
            .from(deployer)
            .to(router)
            .data(Bytes::from(liq_data))
            .value(eth_amount);
        match provider.send_transaction(liq_tx, None).await {
            Ok(pending) => eprintln!("[ANVIL] addLiquidityETH sent: {:?}", pending.tx_hash()),
            Err(e) => eprintln!("[ANVIL] addLiquidityETH failed: {}", e),
        }

        drop(provider);
        drop(repo);

        // Mine to confirm
        let repo = self.repository.read().await;
        repo.mine(identifier.to_string()).await;
        eprintln!("[ANVIL] addLiquidityETH mined");
    }

    async fn cleanup(&self, identifier: &str) {
        let mut repo = self.repository.write().await;
        repo.remove_anvil_instance(identifier);
    }
}
