#!/bin/bash

set -euo pipefail

N=${1:?Usage: research_loop.sh <iterations> [session_id]}
SESSION_ID=${2:-}

# ── Colors & symbols ──────────────────────────────────────────────
BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[32m'
CYAN='\033[36m'
YELLOW='\033[33m'
RED='\033[31m'
RESET='\033[0m'
SPINNER_CHARS='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'

# ── Session setup ─────────────────────────────────────────────────
if [ -z "${SESSION_ID}" ]; then
    printf "${CYAN}Creating new agent session...${RESET}\n"
    SESSION_ID=$(am new ~/code/compression -t codex --yolo --print-session)
    printf "${GREEN}Session: ${BOLD}${SESSION_ID}${RESET}\n"
fi

# ── Helpers ───────────────────────────────────────────────────────
START_TIME=$(date +%s)

elapsed() {
    local secs=$(( $(date +%s) - START_TIME ))
    printf '%02d:%02d:%02d' $((secs/3600)) $((secs%3600/60)) $((secs%60))
}

iter_elapsed() {
    local secs=$(( $(date +%s) - ITER_START ))
    printf '%02d:%02d' $((secs/60)) $((secs%60))
}

# Spinner runs in background; call stop_spinner to kill it
start_spinner() {
    local msg="$1"
    (
        i=0
        while true; do
            c=${SPINNER_CHARS:$((i % ${#SPINNER_CHARS})):1}
            printf "\r  ${CYAN}${c}${RESET} ${DIM}%s  [%s]${RESET}  " "$msg" "$(iter_elapsed)"
            sleep 0.15
            i=$((i + 1))
        done
    ) &
    SPINNER_PID=$!
}

stop_spinner() {
    if [ -n "${SPINNER_PID:-}" ]; then
        kill "$SPINNER_PID" 2>/dev/null || true
        wait "$SPINNER_PID" 2>/dev/null || true
        unset SPINNER_PID
        printf "\r\033[K"  # clear spinner line
    fi
}
trap stop_spinner EXIT

# ── Progress bar ──────────────────────────────────────────────────
progress_bar() {
    local current=$1 total=$2 width=30
    local filled=$(( current * width / total ))
    local empty=$(( width - filled ))
    local pct=$(( current * 100 / total ))
    local bar=""
    for ((j=0; j<filled; j++)); do bar+="█"; done
    for ((j=0; j<empty;  j++)); do bar+="░"; done
    printf "${BOLD}%s${RESET} %3d%%" "$bar" "$pct"
}

# ── Header ────────────────────────────────────────────────────────
TITLE=$(printf "Research Loop  ·  %d iterations" "$N")
BOX_W=$(( ${#TITLE} + 6 ))
BORDER=$(printf '═%.0s' $(seq 1 "$BOX_W"))
printf "\n${BOLD}╔%s╗${RESET}\n" "$BORDER"
printf "${BOLD}║   ${CYAN}%s${RESET}${BOLD}   ║${RESET}\n" "$TITLE"
printf "${BOLD}╚%s╝${RESET}\n\n" "$BORDER"

# ── Main loop ─────────────────────────────────────────────────────
PASS=0 FAIL=0

for i in $(seq 1 "${N}"); do
    ITER_START=$(date +%s)

    printf "${BOLD}[%d/%d]${RESET}  %s  ${DIM}elapsed %s${RESET}\n" "$i" "$N" "$(progress_bar $((i-1)) "$N")" "$(elapsed)"

    # Phase 1: reset context
    printf "  ${YELLOW}↻${RESET} Resetting context...\n"
    start_spinner "Waiting for /new"
    am wait --state waiting_input,idle "${SESSION_ID}"
    if am send "${SESSION_ID}" '/new'; then
        stop_spinner
        printf "  ${GREEN}✓${RESET} Context reset\n"
    else
        stop_spinner
        printf "  ${RED}✗${RESET} Context reset failed — skipping iteration\n"
        FAIL=$((FAIL + 1))
        continue
    fi

    # Phase 2: run autoresearch
    start_spinner "Running autoresearch"
    am wait --state waiting_input,idle "${SESSION_ID}"
    if am send "${SESSION_ID}" '$compression-autoresearch'; then
        stop_spinner
        printf "  ${GREEN}✓${RESET} Autoresearch complete  ${DIM}(%s)${RESET}\n" "$(iter_elapsed)"
        PASS=$((PASS + 1))
    else
        stop_spinner
        printf "  ${RED}✗${RESET} Autoresearch failed    ${DIM}(%s)${RESET}\n" "$(iter_elapsed)"
        FAIL=$((FAIL + 1))
    fi

    printf "\n"
done

# ── Summary ───────────────────────────────────────────────────────
TOTAL_SECS=$(( $(date +%s) - START_TIME ))
AVG=$(( N > 0 ? TOTAL_SECS / N : 0 ))

printf "${BOLD}────────────────────────────────────────────${RESET}\n"
printf "  %s  ${BOLD}Done!${RESET}\n\n" "$(progress_bar "$N" "$N")"
printf "  ${GREEN}Passed:${RESET}  %d/${N}\n" "$PASS"
if [ "$FAIL" -gt 0 ]; then
    printf "  ${RED}Failed:${RESET}  %d/${N}\n" "$FAIL"
fi
printf "  ${CYAN}Total:${RESET}   $(elapsed)\n"
printf "  ${DIM}Avg:     %02d:%02d per iteration${RESET}\n" $((AVG/60)) $((AVG%60))
printf "${BOLD}────────────────────────────────────────────${RESET}\n"
