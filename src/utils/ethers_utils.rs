use ethers::abi::Token;
use serde_json::json;

pub struct EthersUtils;

impl EthersUtils {
    pub fn token_to_json(token: Token) -> serde_json::Value {
        match token {
            Token::String(s) => json!(s),
            Token::Uint(u) => json!(u.to_string()),
            Token::Int(i) => json!(i.to_string()),
            Token::Bool(b) => json!(b),
            Token::Address(addr) => json!(format!("{:?}", addr)),
            Token::Bytes(bytes) => json!(bytes),
            _ => json!(format!("{:?}", token)),
        }
    }

}