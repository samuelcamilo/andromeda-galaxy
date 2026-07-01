use heimdall_cfg::{cfg, CfgArgsBuilder };
use serde::Serialize;

pub struct HeimdallService {}

#[derive(Debug, Serialize)]
pub struct Weight {
    pub id: String,
    pub op: String,
    pub value: Option<String>,
}
impl HeimdallService {
    async fn get_cfg<'a>(&self, bytecode: String) -> Result<Vec<String>, ()> {
        let args = CfgArgsBuilder::new()
            .target(bytecode.to_string())
            .build()
            .map_err(|e| eprintln!("[HEIMDALL] Failed to build CFG args: {:?}", e))?;

        let result = cfg(args).await
            .map_err(|e| eprintln!("[HEIMDALL] Failed to generate CFG: {:?}", e))?;

        let node_weights: Vec<String> = result.graph.node_weights().cloned().collect();
        Ok(node_weights)
    }

    pub async fn get_cfg_as_json(&self, bytecode:String) -> Result<Vec<Vec<Weight>>, ()> {
        let cfg = self.get_cfg(bytecode).await?;
        // Processa uma lista de strings de CFG e retorna um vetor de objetos Weight
        let mut weights = Vec::new();

        for (index, weight) in cfg.iter().enumerate() {
            // Formata a string do peso baseado no índice (e.g., adiciona quebra de linha no índice 0, se necessário)
            let formatted_weight = self.format_weight(index, weight);

            // Extrai as instruções não vazias da string formatada
            let instructions = self.extract_instructions(&formatted_weight);

            let mut current_weight = Vec::new();

            // Itera sobre cada instrução extraída
            for instruction in instructions {
                // Transforma a instrução em um objeto Weight, se possível
                if let Some(weight_obj) = self.parse_instruction(&instruction) {
                    current_weight.push(weight_obj); // Adiciona ao vetor final
                }
            }

            weights.push(current_weight);
        }

        // Retorna o vetor final de objetos Weight
        Ok(weights)
    }

    // Formata o peso com base no índice
    fn format_weight(&self, index: usize, weight: &str) -> String {
        index
            .eq(&0)
            .then(|| format!("\n{}", weight)) // Adiciona quebra de linha para o primeiro índice
            .unwrap_or_else(|| weight.to_string())
    }

    // Extrai instruções de uma string formatada
    fn extract_instructions<'a>(&self, weight: &'a str) -> Vec<&'a str> {
        weight
            .split('\n') // Divide a string por novas linhas
            .filter(|line| !line.trim().is_empty()) // Filtra linhas vazias
            .collect()
    }

    // Faz o parsing de uma instrução para transformar em um objeto Weight
    fn parse_instruction(&self, instruction: &str) -> Option<Weight> {
        let parts: Vec<&str> = instruction.split_whitespace().collect();

        if parts.len() < 2 {
            return None;
        }

        Some(Weight {
            id: parts[0].to_string(),
            op: parts[1].to_string(),
            value: if parts.len() >= 3 { Some(parts[2].to_string()) } else { None },
        })
    }

}