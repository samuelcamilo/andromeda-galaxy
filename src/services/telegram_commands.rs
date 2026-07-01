use crate::http_client::HttpClient;
use crate::repositories::ethers::ethers_repository::EthersRepository;
use crate::repositories::sqlite_repository::SqliteRepository;
use crate::services::enrichment_service::{CompareReport, EnrichmentService, SimilarMatch};
use crate::services::telegram_service::TelegramService;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Resposta de um comando: pode ser texto simples (default) ou um card
/// MarkdownV2 (`/check`) que reaproveita o formatador do TelegramService.
pub enum CommandResponse {
    Plain(String),
    MarkdownV2(String),
}

pub struct TelegramCommands {
    bot_token: Arc<RwLock<Option<String>>>,
    poll_started: Arc<AtomicBool>,
    sqlite_repository: Arc<SqliteRepository>,
    enrichment_service: Arc<EnrichmentService>,
    ethers_repository: Arc<RwLock<EthersRepository>>,
    telegram_service: Arc<TelegramService>,
}

impl TelegramCommands {
    pub fn new(
        sqlite_repository: Arc<SqliteRepository>,
        enrichment_service: Arc<EnrichmentService>,
        ethers_repository: Arc<RwLock<EthersRepository>>,
        telegram_service: Arc<TelegramService>,
    ) -> Self {
        TelegramCommands {
            bot_token: Arc::new(RwLock::new(None)),
            poll_started: Arc::new(AtomicBool::new(false)),
            sqlite_repository,
            enrichment_service,
            ethers_repository,
            telegram_service,
        }
    }

    pub async fn configure(&self, bot_token: String) {
        *self.bot_token.write().await = Some(bot_token.clone());
        if self
            .poll_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            eprintln!("[TELEGRAM_CMD] Poll loop ja esta ativo; token atualizado");
            return;
        }

        let token = self.bot_token.clone();
        let repo = self.sqlite_repository.clone();
        let enrichment = self.enrichment_service.clone();
        let ethers = self.ethers_repository.clone();
        let telegram = self.telegram_service.clone();
        tokio::spawn(Self::poll_loop(token, repo, enrichment, ethers, telegram));
    }

    async fn poll_loop(
        bot_token: Arc<RwLock<Option<String>>>,
        repo: Arc<SqliteRepository>,
        enrichment_service: Arc<EnrichmentService>,
        ethers_repository: Arc<RwLock<EthersRepository>>,
        telegram_service: Arc<TelegramService>,
    ) {
        let mut offset: i64 = 0;
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(45))
            .build()
            .unwrap_or_else(|_| HttpClient::new().get_client().clone());

        loop {
            let token = {
                let lock = bot_token.read().await;
                match lock.as_ref() {
                    Some(t) => t.clone(),
                    None => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }
            };

            let url = format!(
                "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout=30",
                token, offset
            );

            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[TELEGRAM_CMD] Erro ao buscar updates: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                eprintln!(
                    "[TELEGRAM_CMD] getUpdates falhou: status={} body={}",
                    status, body
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                continue;
            }

            let body: serde_json::Value = match resp.json().await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[TELEGRAM_CMD] Erro parseando updates: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            let updates = match body.get("result").and_then(|r| r.as_array()) {
                Some(arr) => arr.clone(),
                None => {
                    eprintln!("[TELEGRAM_CMD] getUpdates sem result: {}", body);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            for update in &updates {
                let update_id = update.get("update_id").and_then(|u| u.as_i64()).unwrap_or(0);
                offset = update_id + 1;

                let message = match update.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let text = match message.get("text").and_then(|t| t.as_str()) {
                    Some(t) => t.to_string(),
                    None => continue,
                };

                let chat_id = match message.get("chat").and_then(|c| c.get("id")).and_then(|i| i.as_i64()) {
                    Some(id) => id,
                    None => continue,
                };

                let response = Self::handle_command(
                    &repo,
                    &enrichment_service,
                    &ethers_repository,
                    &telegram_service,
                    &text,
                )
                .await;

                match response {
                    Some(CommandResponse::Plain(text_resp)) => {
                        let send_url =
                            format!("https://api.telegram.org/bot{}/sendMessage", token);
                        let send_body = serde_json::json!({
                            "chat_id": chat_id,
                            "text": text_resp,
                        });
                        let _ = client
                            .post(&send_url)
                            .json(&send_body)
                            .send()
                            .await;
                    }
                    Some(CommandResponse::MarkdownV2(text_resp)) => {
                        // Reaproveita a infra do TelegramService que tem
                        // fallback automático pra texto puro caso o parser
                        // do MarkdownV2 reclame de algum entity.
                        let chat_id_str = chat_id.to_string();
                        if let Err(e) = TelegramService::send_message(
                            &token,
                            &chat_id_str,
                            &text_resp,
                        )
                        .await
                        {
                            eprintln!(
                                "[TELEGRAM_CMD] falha enviando MarkdownV2 pra {}: {}",
                                chat_id, e
                            );
                        }
                    }
                    None => {}
                }
            }
        }
    }

    async fn handle_command(
        repo: &Arc<SqliteRepository>,
        enrichment_service: &Arc<EnrichmentService>,
        ethers_repository: &Arc<RwLock<EthersRepository>>,
        telegram_service: &Arc<TelegramService>,
        text: &str,
    ) -> Option<CommandResponse> {
        let parts: Vec<&str> = text.splitn(3, ' ').collect();
        if parts.is_empty() || !parts[0].starts_with('/') {
            return None;
        }

        let command = &parts[0][1..];
        let args = if parts.len() > 1 { parts[1..].join(" ") } else { String::new() };

        // Comandos especiais que retornam MarkdownV2 ou precisam de
        // dependências extras (provider WS / TelegramService).
        match command {
            "check" => {
                let addr = args.split_whitespace().next().unwrap_or("");
                if addr.is_empty() {
                    return Some(CommandResponse::Plain("Uso: /check <ca>".to_string()));
                }
                return Some(
                    Self::handle_check(
                        enrichment_service,
                        ethers_repository,
                        telegram_service,
                        addr,
                    )
                    .await,
                );
            }
            _ => {}
        }

        Self::handle_plain_command(repo, enrichment_service, command, &args)
            .await
            .map(|either| either)
    }

    async fn handle_check(
        enrichment_service: &Arc<EnrichmentService>,
        ethers_repository: &Arc<RwLock<EthersRepository>>,
        telegram_service: &Arc<TelegramService>,
        addr: &str,
    ) -> CommandResponse {
        // Pega o WS provider já conectado (apply_rpc usa user_id=1, igual
        // o resto do sistema). `get_shared_provider` devolve a referência
        // viva — se a WS reconectar, pega a nova automaticamente.
        let provider = {
            let ethers_lock = ethers_repository.read().await;
            match ethers_lock.get_shared_provider(1) {
                Some(p) => p,
                None => {
                    return CommandResponse::Plain(
                        "RPC WS não está conectado (apply_rpc ainda não rodou)".to_string(),
                    )
                }
            }
        };
        let provider_arc = provider.read().await.clone();

        let enriched = match enrichment_service
            .enrich_by_address(provider_arc, addr)
            .await
        {
            Ok(e) => e,
            Err(e) => {
                return CommandResponse::Plain(format!("Erro no /check: {}", e))
            }
        };

        // Bot username vem do TelegramService config (mesmo usado nos
        // botões dos cards normais). Cai pra default se ainda não setou.
        let bot_username = telegram_service
            .current_config()
            .await
            .map(|(_, _, u)| u)
            .unwrap_or_else(|| "deployerethmasterbot".to_string());

        let card = TelegramService::format_message(&enriched, &bot_username);
        CommandResponse::MarkdownV2(card)
    }

    async fn handle_plain_command(
        repo: &Arc<SqliteRepository>,
        enrichment_service: &Arc<EnrichmentService>,
        command: &str,
        args: &str,
    ) -> Option<CommandResponse> {
        let args = args.to_string();
        let plain = match command {
            "add" => {
                let args_parts: Vec<&str> = args.splitn(3, ' ').collect();
                if args_parts.len() >= 2 {
                    let checksum = args_parts[1];
                    let tag = if args_parts.len() > 2 { args_parts[2] } else { "" };
                    if !tag.is_empty() {
                        match repo.set_indicator(checksum, tag) {
                            Ok(_) => Some(format!("Indicator set: {} = {}", checksum, tag)),
                            Err(e) => Some(format!("Error: {}", e)),
                        }
                    } else {
                        Some("Usage: /add <address> <checksum> <tag>".to_string())
                    }
                } else {
                    Some("Usage: /add <address> <checksum> <tag>".to_string())
                }
            }
            "addBatch" => {
                let args_parts: Vec<&str> = args.split_whitespace().collect();
                if args_parts.len() >= 2 {
                    let tag = args_parts.last().unwrap_or(&"");
                    let checksums = &args_parts[1..args_parts.len()-1];
                    let mut count = 0;
                    for ck in checksums {
                        if !ck.is_empty() {
                            let _ = repo.set_indicator(ck, tag);
                            count += 1;
                        }
                    }
                    Some(format!("Batch: {} indicators set with tag '{}'", count, tag))
                } else {
                    Some("Usage: /addBatch <address> <checksums...> <tag>".to_string())
                }
            }
            "del" => {
                if !args.is_empty() {
                    match repo.del_indicator(args.trim()) {
                        Ok(_) => Some(format!("Indicator deleted: {}", args.trim())),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /del <checksum>".to_string())
                }
            }
            "clear" => {
                match repo.clear_indicators() {
                    Ok(_) => Some("All indicators cleared".to_string()),
                    Err(e) => Some(format!("Error: {}", e)),
                }
            }
            "anote" => {
                // Format: /anote <address> <bytecodeChecksum> <functionsChecksum> <text...>
                let args_parts: Vec<&str> = args.splitn(4, ' ').collect();
                if args_parts.len() >= 4 {
                    let bytecode_checksum = args_parts[1];
                    let functions_checksum = args_parts[2];
                    let text = args_parts[3];
                    if text.trim().is_empty() {
                        let _ = repo.del_annotation(bytecode_checksum);
                        let _ = repo.del_annotation(functions_checksum);
                        Some("Annotation cleared".to_string())
                    } else {
                        let _ = repo.set_annotation(bytecode_checksum, text);
                        let _ = repo.set_annotation(functions_checksum, text);
                        Some(format!("Annotation set for {} and {}", bytecode_checksum, functions_checksum))
                    }
                } else {
                    Some("Usage: /anote <address> <bytecodeChecksum> <functionsChecksum> <text>".to_string())
                }
            }
            "anoteGas" => {
                // Format: /anoteGas <address> <bytecodeChecksum> <functionsChecksum> <text...>
                let args_parts: Vec<&str> = args.splitn(4, ' ').collect();
                if args_parts.len() >= 4 {
                    let bytecode_checksum = args_parts[1];
                    let functions_checksum = args_parts[2];
                    let text = args_parts[3];
                    if text.trim().is_empty() {
                        let _ = repo.del_gas_annotation(bytecode_checksum);
                        let _ = repo.del_gas_annotation(functions_checksum);
                        Some("Gas annotation cleared".to_string())
                    } else {
                        let _ = repo.set_gas_annotation(bytecode_checksum, text);
                        let _ = repo.set_gas_annotation(functions_checksum, text);
                        Some(format!("Gas annotation set for {} and {}", bytecode_checksum, functions_checksum))
                    }
                } else {
                    Some("Usage: /anoteGas <address> <bytecodeChecksum> <functionsChecksum> <text>".to_string())
                }
            }
            "anoteAppend" => {
                // Format: /anoteAppend <address> <bytecodeChecksum> <functionsChecksum> <text...>
                let args_parts: Vec<&str> = args.splitn(4, ' ').collect();
                if args_parts.len() >= 4 {
                    let bytecode_checksum = args_parts[1];
                    let functions_checksum = args_parts[2];
                    let text = args_parts[3];
                    let _ = repo.append_annotation(bytecode_checksum, text);
                    let r = repo.append_annotation(functions_checksum, text);
                    match r {
                        Ok(new_text) => Some(format!("Annotation appended: {}", new_text)),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /anoteAppend <address> <bytecodeChecksum> <functionsChecksum> <text>".to_string())
                }
            }
            "anoteGasAppend" => {
                // Format: /anoteGasAppend <address> <bytecodeChecksum> <functionsChecksum> <text...>
                let args_parts: Vec<&str> = args.splitn(4, ' ').collect();
                if args_parts.len() >= 4 {
                    let bytecode_checksum = args_parts[1];
                    let functions_checksum = args_parts[2];
                    let text = args_parts[3];
                    let _ = repo.append_gas_annotation(bytecode_checksum, text);
                    let r = repo.append_gas_annotation(functions_checksum, text);
                    match r {
                        Ok(new_text) => Some(format!("Gas annotation appended: {}", new_text)),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /anoteGasAppend <address> <bytecodeChecksum> <functionsChecksum> <text>".to_string())
                }
            }
            "ignore" => {
                if !args.is_empty() {
                    match repo.add_ignore(args.trim()) {
                        Ok(_) => Some(format!("Ignore added: {}", args.trim())),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /ignore <checksum>".to_string())
                }
            }
            "rmignore" => {
                if !args.is_empty() {
                    match repo.rm_ignore(args.trim()) {
                        Ok(_) => Some(format!("Ignore removed: {}", args.trim())),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /rmignore <checksum>".to_string())
                }
            }
            "setsigma" => {
                if !args.is_empty() {
                    match repo.set_setting("sigma_user", args.trim()) {
                        Ok(_) => Some(format!("Sigma user set to: {}", args.trim())),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /setsigma <username>".to_string())
                }
            }
            "setbanana" => {
                if !args.is_empty() {
                    match repo.set_setting("banana_user", args.trim()) {
                        Ok(_) => Some(format!("Banana user set to: {}", args.trim())),
                        Err(e) => Some(format!("Error: {}", e)),
                    }
                } else {
                    Some("Usage: /setbanana <username>".to_string())
                }
            }
            "compare" => {
                // Dois modos:
                //   - `/compare <addrA> <addrB>` → comparação 1-a-1 via RPC.
                //   - `/compare <addr>`          → busca os deploys mais
                //     parecidos no histórico (`sent_messages`).
                let args_parts: Vec<&str> = args.split_whitespace().collect();
                match args_parts.len() {
                    1 => {
                        let addr = args_parts[0];
                        match enrichment_service.find_similar(addr, 5).await {
                            Ok((target, matches)) => {
                                Some(Self::format_similar_report(&target, &matches))
                            }
                            Err(e) => Some(format!("Erro no /compare: {}", e)),
                        }
                    }
                    n if n >= 2 => {
                        let addr_a = args_parts[0];
                        let addr_b = args_parts[1];
                        match enrichment_service.compare(addr_a, addr_b).await {
                            Ok(report) => Some(Self::format_compare_report(&report)),
                            Err(e) => Some(format!("Erro no /compare: {}", e)),
                        }
                    }
                    _ => Some(
                        "Uso:\n  /compare <addr>            → busca similares no histórico\n  /compare <addrA> <addrB>   → compara 1-a-1"
                            .to_string(),
                    ),
                }
            }
            _ => None,
        };

        plain.map(CommandResponse::Plain)
    }

    /// Render plain-text do modo "busca" (`/compare <addr>`).
    fn format_similar_report(target: &str, matches: &[SimilarMatch]) -> String {
        if matches.is_empty() {
            return format!(
                "🔍 Compare {}\nNenhum contrato similar encontrado no histórico.",
                target
            );
        }

        let mut out = String::new();
        out.push_str(&format!("🔍 Compare {}\n", target));
        out.push_str(&format!("Top {} similares no histórico:\n\n", matches.len()));

        for (i, m) in matches.iter().enumerate() {
            let pct = (m.score * 100.0).round() as u32;
            let badge = match pct {
                100 => "🟢 LOGIC MATCH",
                95..=99 => "🟢 quase idêntico",
                80..=94 => "🟡 muito similar",
                50..=79 => "🟠 parcial",
                _ => "🔴 fraco",
            };

            let title = match (m.name.as_deref(), m.symbol.as_deref()) {
                (Some(n), Some(s)) => format!("{} ({})", n, s),
                (Some(n), None) => n.to_string(),
                (None, Some(s)) => s.to_string(),
                _ => "?".to_string(),
            };

            let scam_tag = if m.is_scam { " ❌RUGGED" } else { "" };
            let bc_tag = if m.bc_checksum_match { " ✅BC" } else { "" };
            let fn_tag = if m.fn_checksum_match { " ✅FN" } else { "" };

            // ATH MCAP (FDV) via GeckoTerminal. Quando o token não tem
            // pools/dados, mostra "n/d" em vez de esconder — ajuda a
            // distinguir "não busquei" de "não existe pool".
            let ath_str = match m.ath_market_cap_usd {
                Some(v) if v > 0.0 => {
                    let when = m
                        .ath_at
                        .map(Self::format_relative_age)
                        .unwrap_or_default();
                    format!(
                        "   ATH MCAP: {} {}\n",
                        Self::format_usd_compact(v),
                        when
                    )
                }
                _ => "   ATH MCAP: n/d\n".to_string(),
            };

            out.push_str(&format!(
                "{}. {}% {} — {}{}\n   {}\n   Seletores: {}/{} (jaccard {:.0}%){}{}\n{}",
                i + 1,
                pct,
                badge,
                title,
                scam_tag,
                m.address,
                m.selectors_shared,
                m.selectors_total_target.max(m.selectors_total_candidate),
                m.selectors_jaccard * 100.0,
                bc_tag,
                fn_tag,
                ath_str,
            ));
        }

        out
    }

    /// `1234567890.0` → `"$1.23M"`, `980000.0` → `"$980K"`, etc.
    /// Formato compacto pra caber bem na linha do `/compare`.
    fn format_usd_compact(v: f64) -> String {
        if v >= 1_000_000_000.0 {
            format!("${:.2}B", v / 1_000_000_000.0)
        } else if v >= 1_000_000.0 {
            format!("${:.2}M", v / 1_000_000.0)
        } else if v >= 1_000.0 {
            format!("${:.0}K", v / 1_000.0)
        } else {
            format!("${:.0}", v)
        }
    }

    /// Unix seconds → "(há 3d)" / "(há 2mo)" / "(há 1y)". Aproximações
    /// suficientes pro contexto (sabermos a ordem de grandeza).
    fn format_relative_age(ts: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let diff = (now - ts).max(0);
        const DAY: i64 = 86_400;
        if diff < DAY {
            "(hoje)".to_string()
        } else if diff < 30 * DAY {
            format!("(há {}d)", diff / DAY)
        } else if diff < 365 * DAY {
            format!("(há {}mo)", diff / (30 * DAY))
        } else {
            format!("(há {}y)", diff / (365 * DAY))
        }
    }

    /// Render plain-text do `CompareReport`. Mantemos sem MarkdownV2 pra
    /// não brigar com o parser do Telegram (o `sendMessage` aqui não passa
    /// `parse_mode`, então caracteres especiais saem literais).
    fn format_compare_report(r: &CompareReport) -> String {
        let pct_overall = (r.overall_similarity * 100.0).round() as u32;
        let badge = match pct_overall {
            100 => "🟢 IDÊNTICOS",
            95..=99 => "🟢 quase idênticos",
            80..=94 => "🟡 muito similares",
            50..=79 => "🟠 parcialmente similares",
            _ => "🔴 distintos",
        };

        let bc_match_str = if r.bytecode_identical {
            "✅ idêntico (byte a byte)".to_string()
        } else if r.bc_checksum_match {
            "✅ MATCH (mesma lógica Heimdall)".to_string()
        } else {
            "❌ diferente".to_string()
        };

        let fn_match_str = if r.fn_checksum_match {
            "✅ MATCH (mesmo conjunto de seletores)".to_string()
        } else {
            "❌ diferente".to_string()
        };

        let len_diff_pct = if r.bytecode_len_a == r.bytecode_len_b {
            "0%".to_string()
        } else {
            let max = r.bytecode_len_a.max(r.bytecode_len_b) as f64;
            let diff = (r.bytecode_len_a as f64 - r.bytecode_len_b as f64).abs();
            format!("{:.1}%", (diff / max) * 100.0)
        };

        format!(
            "🔍 Compare {} ({}%)\n\
             A: {}\n\
             B: {}\n\
             \n\
             • Bytecode: {}\n\
             • Bytecode checksum: {} ↔ {}\n\
             • Functions checksum: {} ↔ {}\n\
                └ {}\n\
             • Seletores: {} compartilhados (A={}, B={}) — Jaccard {:.1}%\n\
             • Opcodes (4-shingle Jaccard): {:.1}%\n\
             • Tamanho: {} vs {} bytes (Δ {})\n\
             \n\
             Score consolidado: {}% {}",
            badge,
            pct_overall,
            r.address_a,
            r.address_b,
            bc_match_str,
            r.bc_checksum_a,
            r.bc_checksum_b,
            r.fn_checksum_a,
            r.fn_checksum_b,
            fn_match_str,
            r.selectors_shared,
            r.selectors_total_a,
            r.selectors_total_b,
            r.selectors_jaccard * 100.0,
            r.opcode_jaccard * 100.0,
            r.bytecode_len_a,
            r.bytecode_len_b,
            len_diff_pct,
            pct_overall,
            badge,
        )
    }
}
