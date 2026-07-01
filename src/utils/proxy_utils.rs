use std::str::FromStr;
use ethers::types::H160;

pub struct ProxyUtils;

impl ProxyUtils {

    /// Verifica se o bytecode é um proxy EIP-1167 e extrai o endereço da implementação.
    fn extract_proxy_address_by_eip1167(bytecode: &str) -> Option<H160> {
        // Remove o prefixo '0x' se existir
        let clean_bytecode = bytecode.trim_start_matches("0x");

        // O padrão correto do EIP-1167 inclui o opcode 0x363d3d373d3d3d363d73 seguido de um endereço de 20 bytes (40 caracteres hex)
        if let Some(pos) = clean_bytecode.find("363d3d373d3d3d363d73") {
            let start = pos + 20; // Pular os 10 bytes de opcode
            if clean_bytecode.len() >= start + 40 {
                let address = &clean_bytecode[start..start + 40];
                return H160::from_str(address).ok();
            }
        }
        None
    }

    pub fn detect_proxy_and_extract_address(bytecode: &str) -> Option<H160> {
        Self::extract_proxy_address_by_eip1167(bytecode)
    }

}