#!/bin/bash
# Wait for the server to be ready
echo "[STARTUP] Aguardando servidor..."
until curl -s http://localhost:8080/health > /dev/null 2>&1; do
    sleep 1
done
echo "[STARTUP] Servidor disponivel"

# 1. Connect to Ethereum WebSocket RPC
WS_ENDPOINT="${RPC_ENDPOINT:-wss://ethereum-rpc.publicnode.com}"
echo "[STARTUP] Conectando ao RPC Ethereum..."
curl -s -X POST http://localhost:8080/ethers/1/apply_rpc \
  -o /dev/null \
  -H "Content-Type: application/json" \
  --data-binary @- <<EOF
{"endpoint":"${WS_ENDPOINT}","identifier":"eth-mainnet","listen_deploy_event":true}
EOF
echo ""

sleep 2

# 2. Configure Telegram
: "${TELEGRAM_BOT_TOKEN:?TELEGRAM_BOT_TOKEN is required}"
: "${TELEGRAM_CHAT_ID:?TELEGRAM_CHAT_ID is required}"
echo "[STARTUP] Configurando Telegram..."
curl -s -X POST http://localhost:8080/telegram/configure \
  -o /dev/null \
  -H "Content-Type: application/json" \
  --data-binary @- <<EOF
{"bot_token":"${TELEGRAM_BOT_TOKEN}","chat_id":"${TELEGRAM_CHAT_ID}","bot_username":"${TELEGRAM_BOT_USERNAME:-deployerethmasterbot}","etherscan_api_key":"${ETHERSCAN_API_KEY}"}
EOF
echo ""

sleep 2

# 3. Start ERC-20 deploy listener
echo "[STARTUP] Ativando listener de deploys ERC-20..."
curl -s -X POST http://localhost:8080/ethers/1/listen_deploy_erc20 \
  -o /dev/null \
  -H "Content-Type: application/json" \
  -d '{"webhook": "http://localhost:8080/health"}'
echo ""

echo "[STARTUP] Configuracao completa!"
