use serde::{Deserialize, Serialize};

#[derive(Deserialize,Serialize)]
pub struct GetCFGDTO {
    pub(crate)bytecode:String
}