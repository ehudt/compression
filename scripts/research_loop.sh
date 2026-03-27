#!/bin/bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: research_loop.sh <iterations> [session_id] [--reset-every N]

Runs the compression autoresearch skill in a persistent agent session.

Arguments:
  <iterations>          Number of loop iterations to run
  [session_id]          Reuse an existing agent session instead of creating one

Options:
  --reset-every N       Send /new before every Nth iteration after the first.
                        Default: 0 (keep one warm session for the campaign)
EOF
}

if [ $# -lt 1 ]; then
    usage
    exit 1
fi

N=$1
shift

SESSION_ID=""
RESET_EVERY=0

while [ $# -gt 0 ]; do
    case "$1" in
        --reset-every)
            [ $# -ge 2 ] || { usage; exit 1; }
            RESET_EVERY=$2
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            if [ -z "${SESSION_ID}" ]; then
                SESSION_ID=$1
                shift
            else
                usage
                exit 1
            fi
            ;;
    esac
done

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

python3 scripts/autoresearch_log.py ensure-files --results results.tsv --campaigns campaigns.tsv >/dev/null

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

current_head() {
    git rev-parse --verify HEAD 2>/dev/null || true
}

last_subject() {
    git log -1 --pretty=%s 2>/dev/null || true
}

load_summary() {
    local commit="$1"
    local summary
    summary=$(python3 scripts/autoresearch_log.py summarize-commit --results results.tsv --commit "$commit" 2>/dev/null || true)
    SUMMARY_FOUND=0
    SUMMARY_OUTCOME="no_rows"
    SUMMARY_DETAIL=""
    while IFS='=' read -r key value; do
        case "$key" in
            found) SUMMARY_FOUND="$value" ;;
            primary_outcome) SUMMARY_OUTCOME="$value" ;;
            detail) SUMMARY_DETAIL="$value" ;;
        esac
    done <<< "$summary"
}

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
        printf "\r\033[K"
    fi
}
trap stop_spinner EXIT

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

reset_context() {
    local reason="$1"
    printf "  ${YELLOW}↻${RESET} Resetting context (%s)...\n" "$reason"
    start_spinner "Waiting for /new"
    am wait --state waiting_input,idle "${SESSION_ID}"
    if am send "${SESSION_ID}" '/new'; then
        stop_spinner
        printf "  ${GREEN}✓${RESET} Context reset\n"
        return 0
    fi
    stop_spinner
    printf "  ${RED}✗${RESET} Context reset failed\n"
    return 1
}

print_outcome() {
    local outcome="$1"
    local detail="$2"
    case "${outcome}" in
        keep)
            printf "  ${GREEN}●${RESET} Research outcome: kept change"
            ;;
        discard)
            printf "  ${YELLOW}●${RESET} Research outcome: discard batch"
            ;;
        blocked)
            printf "  ${YELLOW}●${RESET} Research outcome: blocked"
            ;;
        inconclusive|commit)
            printf "  ${YELLOW}●${RESET} Research outcome: committed without classified keep/discard"
            ;;
        no_commit)
            printf "  ${RED}●${RESET} Research outcome: no recorded commit"
            ;;
        crash)
            printf "  ${RED}●${RESET} Research outcome: agent crash"
            ;;
    esac
    if [ -n "${detail}" ]; then
        printf "  ${DIM}%s${RESET}" "${detail}"
    fi
    printf "\n"
}

# ── Header ────────────────────────────────────────────────────────
TITLE=$(printf "Research Loop  ·  %d iterations" "$N")
BOX_W=$(( ${#TITLE} + 6 ))
BORDER=$(printf '═%.0s' $(seq 1 "$BOX_W"))
printf "\n${BOLD}╔%s╗${RESET}\n" "$BORDER"
printf "${BOLD}║   ${CYAN}%s${RESET}${BOLD}   ║${RESET}\n" "$TITLE"
printf "${BOLD}╚%s╝${RESET}\n\n" "$BORDER"

# ── Main loop ─────────────────────────────────────────────────────
PASS=0
FAIL=0
KEEP_BATCHES=0
DISCARD_BATCHES=0
BLOCKED_BATCHES=0
OTHER_COMMITS=0
NO_COMMIT_BATCHES=0

printf "${DIM}Campaign reset cadence:${RESET} "
if [ "${RESET_EVERY}" -gt 0 ]; then
    printf "every %d iteration(s)\n\n" "${RESET_EVERY}"
else
    printf "persistent session (no forced resets)\n\n"
fi

for i in $(seq 1 "${N}"); do
    ITER_START=$(date +%s)

    printf "${BOLD}[%d/%d]${RESET}  %s  ${DIM}elapsed %s${RESET}\n" "$i" "$N" "$(progress_bar $((i-1)) "$N")" "$(elapsed)"

    if [ "${RESET_EVERY}" -gt 0 ] && [ "$i" -gt 1 ] && [ $(( (i - 1) % RESET_EVERY )) -eq 0 ]; then
        if ! reset_context "campaign boundary"; then
            FAIL=$((FAIL + 1))
            print_outcome "crash" "reset failed before iteration"
            printf "\n"
            continue
        fi
    fi

    HEAD_BEFORE=$(current_head)

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
        print_outcome "crash" ""
        printf "\n"
        continue
    fi

    HEAD_AFTER=$(current_head)
    SUBJECT=$(last_subject)
    OUTCOME="no_commit"
    DETAIL=""

    if [ -n "${HEAD_BEFORE}" ] && [ -n "${HEAD_AFTER}" ] && [ "${HEAD_BEFORE}" != "${HEAD_AFTER}" ]; then
        load_summary "${HEAD_AFTER}"
        if [ "${SUMMARY_FOUND}" = "1" ] && [ "${SUMMARY_OUTCOME}" != "no_rows" ]; then
            OUTCOME="${SUMMARY_OUTCOME}"
            DETAIL="${SUMMARY_DETAIL}"
            case "${OUTCOME}" in
                keep) KEEP_BATCHES=$((KEEP_BATCHES + 1)) ;;
                discard) DISCARD_BATCHES=$((DISCARD_BATCHES + 1)) ;;
                blocked) BLOCKED_BATCHES=$((BLOCKED_BATCHES + 1)) ;;
                *) OTHER_COMMITS=$((OTHER_COMMITS + 1)) ;;
            esac
        elif [[ "${SUBJECT}" =~ Update\ autoresearch\ results\ and\ ideas\ \(([0-9]+)\ experiments,\ ([0-9]+)\ kept\) ]]; then
            EXPERIMENTS=${BASH_REMATCH[1]}
            KEPT=${BASH_REMATCH[2]}
            DETAIL="${EXPERIMENTS} experiment(s), ${KEPT} kept"
            if [ "${KEPT}" -gt 0 ]; then
                OUTCOME="keep"
                KEEP_BATCHES=$((KEEP_BATCHES + 1))
            else
                OUTCOME="discard"
                DISCARD_BATCHES=$((DISCARD_BATCHES + 1))
            fi
        elif [[ "${SUBJECT}" =~ blocked|Blocked ]]; then
            OUTCOME="blocked"
            DETAIL="${SUBJECT}"
            BLOCKED_BATCHES=$((BLOCKED_BATCHES + 1))
        else
            OUTCOME="commit"
            DETAIL="${SUBJECT}"
            OTHER_COMMITS=$((OTHER_COMMITS + 1))
        fi
    else
        NO_COMMIT_BATCHES=$((NO_COMMIT_BATCHES + 1))
    fi

    print_outcome "${OUTCOME}" "${DETAIL}"
    printf "\n"
done

# ── Summary ───────────────────────────────────────────────────────
TOTAL_SECS=$(( $(date +%s) - START_TIME ))
AVG=$(( N > 0 ? TOTAL_SECS / N : 0 ))

printf "${BOLD}────────────────────────────────────────────${RESET}\n"
printf "  %s  ${BOLD}Done!${RESET}\n\n" "$(progress_bar "$N" "$N")"
printf "  ${GREEN}Agent-complete:${RESET}  %d/${N}\n" "$PASS"
if [ "$FAIL" -gt 0 ]; then
    printf "  ${RED}Agent-failed:${RESET}   %d/${N}\n" "$FAIL"
fi
printf "  ${GREEN}Keep batches:${RESET}    %d\n" "$KEEP_BATCHES"
printf "  ${YELLOW}Discard batches:${RESET} %d\n" "$DISCARD_BATCHES"
if [ "$BLOCKED_BATCHES" -gt 0 ]; then
    printf "  ${YELLOW}Blocked batches:${RESET} %d\n" "$BLOCKED_BATCHES"
fi
if [ "$OTHER_COMMITS" -gt 0 ]; then
    printf "  ${YELLOW}Other commits:${RESET}   %d\n" "$OTHER_COMMITS"
fi
if [ "$NO_COMMIT_BATCHES" -gt 0 ]; then
    printf "  ${RED}No-commit runs:${RESET}  %d\n" "$NO_COMMIT_BATCHES"
fi
if [ -f campaigns.tsv ]; then
    printf "  ${DIM}Campaign log:${RESET} campaigns.tsv\n"
fi
printf "  ${CYAN}Total:${RESET}   $(elapsed)\n"
printf "  ${DIM}Avg:     %02d:%02d per iteration${RESET}\n" $((AVG/60)) $((AVG%60))
printf "${BOLD}────────────────────────────────────────────${RESET}\n"
