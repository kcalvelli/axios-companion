#!/usr/bin/env bash
# Mock companion script for integration tests.
# Emits canned stream-json output that mimics real claude behavior.
#
# Modes (set via MOCK_MODE env var):
#   normal   — init + assistant + result/success (default)
#   error    — init + result/error
#   crash    — exits non-zero with no stream-json
#   slow     — like normal but sleeps 2s before result (for cancellation tests)

MODE="${MOCK_MODE:-normal}"
SESSION_ID="${MOCK_SESSION_ID:-deadbeef-1234-5678-9abc-def012345678}"

case "$MODE" in
  normal)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"assistant","message":{"content":[{"text":"Hello from "}]}}'
    echo '{"type":"assistant","message":{"content":[{"text":"mock companion."}]}}'
    echo '{"type":"result","subtype":"success","result":"Hello from mock companion."}'
    ;;
  error)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"result","subtype":"error","error":"something went wrong"}'
    ;;
  crash)
    exit 1
    ;;
  slow)
    echo '{"type":"system","subtype":"init","session_id":"'"$SESSION_ID"'","model":"claude-sonnet-4-20250514"}'
    echo '{"type":"assistant","message":{"content":[{"text":"thinking..."}]}}'
    sleep 2
    echo '{"type":"result","subtype":"success","result":"thinking..."}'
    ;;
esac
