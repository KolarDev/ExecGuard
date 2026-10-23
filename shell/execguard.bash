# ExecGuard integration for Bash (4.4+).
# Load it from ~/.bashrc:    source /path/to/execguard.bash
# Bypass for one session:    export EXECGUARD_DISABLE=1

[[ $- == *i* ]] || return 0

# Find execguard once: first on PATH, then in this repo's release build.
if ! EXECGUARD_BIN=$(command -v execguard); then
  _execguard_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  EXECGUARD_BIN=
  for _candidate in "$_execguard_dir/../target/release/execguard" "$_execguard_dir/../target/release/execguard.exe"; do
    [[ -x $_candidate ]] && EXECGUARD_BIN=$_candidate && break
  done
  unset _execguard_dir _candidate
  if [[ -z $EXECGUARD_BIN ]]; then
    echo "ExecGuard: execguard not found. Run 'cargo build --release' or add it to PATH. Commands will NOT be checked." >&2
    return 0
  fi
fi

_execguard_ready=0      # 1 = the next command is the first of a newly typed line
_execguard_cancel=0     # 1 = skip everything left on the current line
_execguard_last_hist=
_execguard_last_exit=0  # exit status of the command that just finished

# Set by _execguard_debug just before it lets a line run; read and cleared
# by _execguard_prompt once that line has finished, to log it to history.
_execguard_pending_command=
_execguard_pending_outcome=
_execguard_pending_started_ms=

_execguard_say() {
  [[ -n $1 ]] && printf '%s\n' "$1" >&2
}

# Milliseconds since the Unix epoch. Prefers the shell builtin (Bash 5+,
# no extra process); falls back to the `date` command on older Bash.
_execguard_now_ms() {
  if [[ -n ${EPOCHREALTIME:-} ]]; then
    local whole=${EPOCHREALTIME%.*}
    local frac=${EPOCHREALTIME#*.}
    # `10#` forces base-10, so a fractional part like "012345" isn't
    # misread as an (invalid) octal number.
    printf '%d' $(( whole * 1000 + 10#${frac:0:3} ))
  else
    date +%s%3N
  fi
}

# Runs FIRST at every prompt, before anything else: capture the exit status
# of the command that just finished, then clear the flags for the next one.
_execguard_reset() {
  _execguard_last_exit=$?
  _execguard_cancel=0
  _execguard_ready=0
}

# Runs LAST at every prompt: log the line that just ran (if any), then get
# ready for the next typed line.
_execguard_prompt() {
  if [[ -n $_execguard_pending_command ]]; then
    local duration=$(( $(_execguard_now_ms) - _execguard_pending_started_ms ))
    "$EXECGUARD_BIN" record --shell bash --cwd "$PWD" \
      --exit-code "$_execguard_last_exit" --duration-ms "$duration" \
      --outcome "$_execguard_pending_outcome" -- "$_execguard_pending_command" \
      >/dev/null 2>&1
    _execguard_pending_command=
  fi

  _execguard_last_hist=$(HISTTIMEFORMAT= builtin history 1)
  _execguard_ready=1
}

# Runs before every command. Returning 1 skips the command.
_execguard_debug() {
  # Never skip or check ExecGuard's own functions.
  [[ $BASH_COMMAND == _execguard_* ]] && return 0

  (( _execguard_cancel )) && return 1
  (( _execguard_ready )) || return 0
  [[ $EXECGUARD_DISABLE == 1 ]] && return 0

  local entry line
  entry=$(HISTTIMEFORMAT= builtin history 1)

  if [[ -n $entry && $entry != "$_execguard_last_hist" ]]; then
    # A new line was typed: check the whole line, as typed.
    line=${entry#"${entry%%[0-9]*}"}
    line=${line#*[0-9][* ]}
    line=${line#"${line%%[![:space:]]*}"}
  elif (( BASH_SUBSHELL > 0 )); then
    # No new line and inside a subshell: this is prompt-building
    # (e.g. the Git branch in PS1), not a typed command.
    return 0
  else
    # Typed, but not saved to history (e.g. HISTCONTROL=ignorespace).
    line=$BASH_COMMAND
  fi

  _execguard_ready=0

  local -a args=(check --shell bash)
  (( EUID == 0 )) && args+=(--elevated)

  local messages code
  messages=$("$EXECGUARD_BIN" "${args[@]}" -- "$line" 2>&1)
  code=$?

  case $code in
    0)
      _execguard_will_run "$line" ALLOW
      return 0 ;;
    10)
      _execguard_say "$messages"
      _execguard_will_run "$line" WARN
      return 0 ;;
    20)
      _execguard_say "$messages"
      local reply
      read -r -n 1 -p 'Run it anyway? [y/N] ' reply </dev/tty
      echo >&2
      if [[ $reply == [yY] ]]; then
        _execguard_will_run "$line" CONFIRM
        return 0
      fi
      _execguard_say 'Not run. Press Up to get the command back.'
      _execguard_cancel=1
      return 1 ;;
    30)
      _execguard_say "$messages"
      _execguard_say 'ExecGuard blocked this command because it is on your reject list.'
      _execguard_cancel=1
      return 1 ;;
    *)
      _execguard_say "$messages"
      _execguard_say "ExecGuard failed (exit code $code); running the command UNCHECKED."
      _execguard_will_run "$line" UNCHECKED
      return 0 ;;
  esac
}

# Records that a line is about to run, so _execguard_prompt can log it to
# history once it finishes and the real exit code/duration are known.
_execguard_will_run() {
  _execguard_pending_command=$1
  _execguard_pending_outcome=$2
  _execguard_pending_started_ms=$(_execguard_now_ms)
}

shopt -s extdebug
trap '_execguard_debug' DEBUG

# Reset runs first and prompt runs last. Skip if already installed (sourced twice).
if [[ ${PROMPT_COMMAND[*]} != *_execguard_prompt* ]]; then
  if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(_execguard_reset "${PROMPT_COMMAND[@]}" _execguard_prompt)
  else
    PROMPT_COMMAND="_execguard_reset${PROMPT_COMMAND:+$'\n'$PROMPT_COMMAND}"$'\n''_execguard_prompt'
  fi
fi

printf '\e[90mExecGuard active (%s)\e[0m\n' "$EXECGUARD_BIN" >&2