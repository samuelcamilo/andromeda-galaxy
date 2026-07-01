use crate::services::ethers::find_deploys::find_deploys_service::{
    FindDeploysPayload, FindDeploysService,
};
use crate::utils::proxy_utils::ProxyUtils;
use ethers::middleware::Middleware;
use ethers::prelude::{Provider, TransactionReceipt, Ws};
use ethers::types::GethTraceFrame::CallTracer;
use ethers::types::{CallFrame, GethDebugBuiltInTracerType, GethDebugTracerType, GethDebugTracingOptions, GethTrace, NameOrAddress, };
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct FindByDebugTrace;

impl FindByDebugTrace {
    pub async fn exec(provider: Arc<Provider<Ws>>, receipt: TransactionReceipt) -> Option<FindDeploysPayload> {

        let trace_options = GethDebugTracingOptions {
            tracer: Some(GethDebugTracerType::BuiltInTracer(
                GethDebugBuiltInTracerType::CallTracer,
            )),
            ..Default::default()
        };

        // Obtendo o trace
        let trace = match provider
            .debug_trace_transaction(receipt.transaction_hash, trace_options)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[TRACE] Erro ao rastrear transação {:?}: {}", receipt.transaction_hash, e);
                return None;
            }
        };

        if let GethTrace::Known(CallTracer(call_frame)) = trace {
            Self::process_frames(provider, receipt, &call_frame).await
        } else {
            println!("O trace retornado não é um CallTracer.");
            None
        }

    }

    async fn process_frames(
        provider: Arc<Provider<Ws>>,
        receipt: TransactionReceipt,
        call_frame: &CallFrame,
    ) -> Option<FindDeploysPayload> {
        let main_call =
            Self::callframe_is_erc20(provider.clone(), receipt.clone(), &call_frame.clone()).await;
        if main_call.is_some() {
            return main_call;
        }

        let sub_calls =
            Self::process_subcalls_frame(provider.clone(), receipt.clone(), call_frame.clone().calls).await;
        if sub_calls.is_some() {
            return sub_calls;
        }

        None
    }

    fn process_subcalls_frame(
        provider: Arc<Provider<Ws>>,
        receipt: TransactionReceipt,
        calls: Option<Vec<CallFrame>>,
    ) -> Pin<Box<dyn Future<Output = Option<FindDeploysPayload>> + Send>> { // Add Send here
        Box::pin(async move {
            if let Some(calls) = calls {
                for subcall in calls {
                    if let Some(payload) =
                        Self::callframe_is_erc20(provider.clone(), receipt.clone(), &subcall).await
                    {
                        return Some(payload);
                    }
                    if let Some(subcalls) = subcall.calls {
                        if let Some(payload) =
                            Self::process_subcalls_frame(provider.clone(), receipt.clone(), Some(subcalls))
                                .await
                        {
                            return Some(payload);
                        }
                    }
                }
            }
            None
        })
    }
    async fn callframe_is_erc20(
        provider: Arc<Provider<Ws>>,
        receipt: TransactionReceipt,
        call_frame: &CallFrame,
    ) -> Option<FindDeploysPayload> {
        if call_frame.typ != "CREATE2" && call_frame.typ != "CREATE" {
        // if call_frame.typ != "CREATE2" {
            return None;
        }

        if let Some(to_address) = &call_frame.to {
            if let NameOrAddress::Address(address) = to_address {

                let proxy_address = ProxyUtils::detect_proxy_and_extract_address(
                    call_frame.input.to_string().as_str(),
                );

                FindDeploysService::validate_and_create_payload(
                    provider.clone(),
                    *address,
                    &receipt,
                    proxy_address
                )
                .await
            } else {
                None
            }
        } else {
            None
        }
    }
}
