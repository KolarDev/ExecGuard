# ExecGuard integration for Zsh.
# Load it from ~/.zshrc:     source /path/to/execguard.zsh
# Bypass for one session:    export EXECGUARD_DISABLE=1

[[ -o interactive ]] || return 0

# For $EPOCHREALTIME, used to time how long a command ran.
zmodload zsh/datetime 2>/dev/null

# Find execguard once: first on PATH, then in this repo's release build.
if (( $+commands[execguard] )); then
  typeset -g EXECGUARD_BIN=$commands[execguard]
else
  typeset -g EXECGUARD_BIN=${${(%):-%x}:A:h}/../target/release/execguard
  if [[ ! -x $EXECGUARD_BIN ]]; then
    print -u2 "ExecGuard: execguard not found. Run 'cargo build --release' or add it to PATH. Commands will NOT be checked."
    return 0
  fi
fi

# Messages waiting to be printed just before the command runs.
typeset -g _execguard_pending=''

# Set by _execguard_accept_line just before it lets a command run; read and
# cleared by _execguard_precmd once that command has finished, to log it.
typeset -g _execguard_history_command=''
typeset -g _execguard_history_outcome=''
typeset -g _execguard_history_started_ms=0
typeset -g _execguard_last_exit=0

# Milliseconds since the Unix epoch.
_execguard_now_ms() {
  if [[ -n $EPOCHREALTIME ]]; then
    local whole=${EPOCHREALTIME%%.*}
    local frac=${EPOCHREALTIME#*.}
    # `10#` forces base-10, so a fractional part like "012345" isn't
    # misread as an (invalid) octal number.
    print -r -- $(( whole * 1000 + 10#${frac[1,3]} ))
  else
    date +%s%3N
  fi
}

# Records that a command is about to run, so _execguard_precmd can log it
# to history once it finishes and the real exit code/duration are known.
_execguard_history_will_run() {
  _execguard_history_command=$1
  _execguard_history_outcome=$2
  _execguard_history_started_ms=$(_execguard_now_ms)
}

_execguard_accept_line() {
  emulate -L zsh
  local cmd=$BUFFER

  # Nothing to check, or ExecGuard switched off for this session.
  if [[ -z ${cmd//[[:space:]]/} || $EXECGUARD_DISABLE == 1 ]]; then
    zle .accept-line
    return
  fi

  local -a args=(check --shell zsh)
  (( EUID == 0 )) && args+=(--elevated)

  local messages code
  messages=$("$EXECGUARD_BIN" "${args[@]}" -- "$cmd" 2>&1)
  code=$?

  case $code in
    0)
      _execguard_history_will_run "$cmd" ALLOW
      zle .accept-line ;;
    10)
      _execguard_pending=$messages
      _execguard_history_will_run "$cmd" WARN
      zle .accept-line ;;
    20)
      zle -M "$messages"$'\n''Run it anyway? [y/N]'
      local key
      read -k 1 key
      zle -M ''
      if [[ $key == [yY] ]]; then
        _execguard_pending=$messages
        _execguard_history_will_run "$cmd" CONFIRM
        zle .accept-line
      else
        zle -M 'Not run. Edit the command, or press Ctrl+C to discard it.'
      fi ;;
    30)
      zle -M "$messages"$'\n''ExecGuard blocked this command because it is on your reject list.' ;;
    *)
      _execguard_pending="$messages"$'\n'"ExecGuard failed (exit code $code); running the command UNCHECKED."
      _execguard_history_will_run "$cmd" UNCHECKED
      zle .accept-line ;;
  esac
}

# Runs after Enter is accepted, right before the command executes.
_execguard_preexec() {
  if [[ -n $_execguard_pending ]]; then
    print -u2 -r -- "$_execguard_pending"
    _execguard_pending=''
  fi
}

# Runs right before each prompt is drawn, i.e. after the previous command
# has fully finished -- the only point where its real exit code is known.
_execguard_precmd() {
  _execguard_last_exit=$?

  if [[ -n $_execguard_history_command ]]; then
    local duration=$(( $(_execguard_now_ms) - _execguard_history_started_ms ))
    "$EXECGUARD_BIN" record --shell zsh --cwd "$PWD" \
      --exit-code "$_execguard_last_exit" --duration-ms "$duration" \
      --outcome "$_execguard_history_outcome" -- "$_execguard_history_command" \
      >/dev/null 2>&1
    _execguard_history_command=''
  fi
}

autoload -Uz add-zsh-hook
add-zsh-hook preexec _execguard_preexec
add-zsh-hook precmd _execguard_precmd
zle -N accept-line _execguard_accept_line

print -P "%F{8}ExecGuard active ($EXECGUARD_BIN)%f"