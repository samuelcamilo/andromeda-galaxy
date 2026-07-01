
//

use ethers::types::Opcode;
use revmasm::types::bytecodes::Bytecodes;
use crate::utils::my_disassembler::my_disassemble;

pub struct BytecodeUtils;

impl BytecodeUtils {

    pub fn erc20_essentials_id(&self) -> [&'static str; 4] {
        ["a9059cbb","dd62ed3e","095ea7b3","23b872dd"]
    }

    pub fn is_not_erc20(&self) -> [&'static str; 2] {
        ["5909c0d5","7464fc3d"]
    }

    pub fn bytecode_is_deploy_erc20(bytecode:String) -> bool{
        let essentials = BytecodeUtils.erc20_essentials_id();
        essentials.iter().all(|&id| bytecode.contains(id))
    }

    pub fn bytecode_is_not_erc20(bytecode:String) -> bool{
        let essentials = BytecodeUtils.is_not_erc20();
        essentials.iter().any(|&id| bytecode.contains(id))
    }

    pub fn bytecode_is_create2(bytecode:String) -> bool {
        let bc1 = Bytecodes::from(bytecode.to_string().replace("0x", ""));
        let instructions = my_disassemble(bc1);
        let names: Vec<_> = instructions.iter().map(|obj| &obj.name).collect();

        Self::is_valid_create2_pattern(names)
    }

    fn is_valid_create2_pattern(opcodes: Vec<&String>) -> bool {
        let mut found_push = false;
        let mut found_create2 = false;

        for opcode in opcodes {
            match opcode.as_str() {
                "PUSH1" | "PUSH32" => found_push = true,
                "CREATE2" => {
                    if found_push {
                        found_create2 = true;
                        break;
                    }
                }
                _ => continue,
            }
        }

        found_create2
    }

}
