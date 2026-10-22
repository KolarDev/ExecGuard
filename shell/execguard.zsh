# ExecGuard integration for Zsh.
# Load it from ~/.zshrc:     source /path/to/execguard.zsh
# Bypass for one session:    export EXECGUARD_DISABLE=1

[[ -o interactive ]] || return 0

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
      zle .accept-line ;;
    10)
      _execguard_pending=$messages
      zle .accept-line ;;
    20)
      zle -M "$messages"$'\n''Run it anyway? [y/N]'
      local key
      read -k 1 key
      zle -M ''
      if [[ $key == [yY] ]]; then
        _execguard_pending=$messages
        zle .accept-line
      else
        zle -M 'Not run. Edit the command, or press Ctrl+C to discard it.'
      fi ;;
    30)
      zle -M "$messages"$'\n''ExecGuard blocked this command because it is on your reject list.' ;;
    *)
      _execguard_pending="$messages"$'\n'"ExecGuard failed (exit code $code); running the command UNCHECKED."
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

autoload -Uz add-zsh-hook
add-zsh-hook preexec _execguard_preexec
zle -N accept-line _execguard_accept_line

print -P "%F{8}ExecGuard active ($EXECGUARD_BIN)%f"