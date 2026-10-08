# gilvt hooks for zsh: OSC 7 (working directory) and OSC 133 (prompt / command marks).
[[ -n "${_GILVT_HOOKS_LOADED-}" ]] && return
typeset -g _GILVT_HOOKS_LOADED=1

_gilvt_urlencode() {
  local LC_ALL=C s="$1" out="" c i
  for (( i = 1; i <= ${#s}; i++ )); do
    c="${s[i]}"
    case "$c" in
      [a-zA-Z0-9/._~-]) out+="$c" ;;
      *) out+=$(printf '%%%02X' "'$c") ;;
    esac
  done
  print -rn -- "$out"
}

# Like _gilvt_urlencode, without a subshell per character (it runs before every command). Encodes
# everything but [A-Za-z0-9/._~-], so `;` (the OSC parameter separator), controls and UTF-8 are escaped.
_gilvt_urlencode_fast() {
  local LC_ALL=C s="$1" out="" c h i
  for (( i = 1; i <= ${#s}; i++ )); do
    c="${s[i]}"
    case "$c" in
      [a-zA-Z0-9/._~-]) out+="$c" ;;
      *) printf -v h '%%%02X' "'$c"; out+="$h" ;;
    esac
  done
  print -rn -- "$out"
}

_gilvt_precmd() {
  local ret=$?
  if [[ -n "${_gilvt_cmd_running-}" ]]; then
    printf '\e]133;D;%s\e\\' "$ret"
    unset _gilvt_cmd_running
  fi
  # /etc/zprofile's path_helper or brew shellenv may reorder PATH (another
  # command of the same name could then win), so move GILVT_BIN_DIR back to the front before every prompt.
  if [[ -n "${GILVT_BIN_DIR-}" && "${path[1]-}" != "$GILVT_BIN_DIR" ]]; then
    path=("$GILVT_BIN_DIR" "${(@)path:#$GILVT_BIN_DIR}")
  fi
  printf '\e]7;file://%s%s\e\\' "$HOST" "$(_gilvt_urlencode "$PWD")"
  printf '\e]133;A\e\\'
  [[ -n "${_gilvt_agents_checked-}" ]] || _gilvt_define_agents
  [[ -n "${_gilvt_ssh_checked-}" ]] || _gilvt_define_ssh
  # Keep the input-start marker hook last so it runs after prompt themes rebuild PS1
  # (takes effect from the next prompt: zsh iterates a copy of the array).
  precmd_functions=(${precmd_functions:#_gilvt_mark_input} _gilvt_mark_input)
  return $ret
}

_gilvt_mark_input() {
  [[ "$PS1" == *$'\e]133;B'* ]] || PS1="$PS1%{"$'\e]133;B\e\\'"%}"
}

_gilvt_preexec() {
  typeset -g _gilvt_cmd_running=1
  # $1 is the line as typed; cut to 2000 bytes (LC_ALL=C: zsh indexes bytes, not characters) so the OSC
  # stays under gilvt's 8 KB limit even when every byte is escaped. The cut may split a character; gilvt
  # drops the incomplete tail.
  local LC_ALL=C
  printf '\e]133;C;cmdline_url=%s\e\\' "$(_gilvt_urlencode_fast "${1[1,2000]}")"
}

# Agent wrappers: inside gilvt, `claude` / `codex` (and wrappers listed in GILVT_CODEX_COMMANDS) run with gilvt's hooks added by
# `gilvt hook <kind>-args` (NUL-separated argv). Defined once, at the first prompt (after the
# user's startup files), only for commands that exist and that are not already an alias or
# function. GILVT_CLAUDE_COMMANDS / GILVT_CODEX_COMMANDS (space-separated) choose the names;
# GILVT_NO_AGENT_WRAPPERS=1 opts out. From your own alias or function, call
# `gilvt_agent claude|codex <command> [args…]`.
gilvt_agent() {
  emulate -L zsh
  local kind=$1 cmd=$2 bin=${GILVT_BIN_DIR-}/gilvt out
  shift 2
  if [[ -n "${GILVT_SOCKET-}" && -n "${GILVT_BIN_DIR-}" && -x "$bin" ]] \
      && out=$("$bin" hook "$kind-args" -- "$@" 2>/dev/null) && [[ -n "$out" ]]; then
    out=${out%$'\0'}
    local -a argv_
    argv_=("${(@0)out}")
    command "$cmd" "${argv_[@]}"
  else
    command "$cmd" "$@"
  fi
}

_gilvt_define_agents() {
  emulate -L zsh
  typeset -g _gilvt_agents_checked=1
  [[ -z "${GILVT_NO_AGENT_WRAPPERS-}" && -n "${GILVT_SOCKET-}" && -x "${GILVT_BIN_DIR-}/gilvt" ]] || return 0
  local kind name names
  for kind in claude codex; do
    names=${GILVT_CLAUDE_COMMANDS-claude}
    [[ $kind == codex ]] && names=${GILVT_CODEX_COMMANDS-codex}
    for name in ${=names}; do
      [[ -n $name && $name != *[^A-Za-z0-9._+-]* ]] || continue
      (( $+aliases[$name] || $+functions[$name] )) && continue
      (( $+commands[$name] )) || continue
      eval "function $name { gilvt_agent $kind $name \"\$@\" }"
    done
  done
}

# ssh: interactive logins go through `gilvt ssh` (remote features); GILVT_SSH=0 or `command ssh` bypass it.
# Defined at the first prompt (this file loads from .zshenv, before .zshrc), so a user's own ssh alias or function wins.
_gilvt_define_ssh() {
  emulate -L zsh
  typeset -g _gilvt_ssh_checked=1
  (( $+aliases[ssh] || $+functions[ssh] )) && return 0
  function ssh {
    if [[ -n ${GILVT_SOCKET-} && ${GILVT_SSH-} != 0 && -x ${GILVT_BIN_DIR-}/gilvt ]]; then
      "$GILVT_BIN_DIR/gilvt" ssh -- "$@"
    else
      command ssh "$@"
    fi
  }
}

# Run first so $? is the user's command status, before any theme hook runs.
precmd_functions=(_gilvt_precmd ${precmd_functions[@]} _gilvt_mark_input)
preexec_functions+=(_gilvt_preexec)
