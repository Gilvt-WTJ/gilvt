# gilvt shell integration (bash). gilvt starts bash with `--rcfile` pointing here, so bash is not a
# login shell: emulate login-shell startup first, then install gilvt's hooks.
if [ -z "${__gilvt_startup_done-}" ]; then
  __gilvt_startup_done=1
  [ -r /etc/profile ] && . /etc/profile
  if [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
  elif [ -r "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
  elif [ -r "$HOME/.profile" ]; then . "$HOME/.profile"
  fi
fi

__gilvt_urlencode() {
  local LC_ALL=C s="$1" out="" c i
  for (( i = 0; i < ${#s}; i++ )); do
    c="${s:i:1}"
    case "$c" in
      [a-zA-Z0-9/._~-]) out="$out$c" ;;
      # bash 3.2 sign-extends bytes >= 0x80, hence the mask.
      *) out="$out$(printf '%%%02X' $(( $(printf '%d' "'$c") & 255 )))" ;;
    esac
  done
  printf '%s' "$out"
}

# No subshell per character (runs before every command); bash 3.2 sign-extends bytes >= 0x80, hence the mask.
__gilvt_urlencode_fast() {
  local LC_ALL=C s="$1" out="" c n h i
  for (( i = 0; i < ${#s}; i++ )); do
    c="${s:i:1}"
    case "$c" in
      [a-zA-Z0-9/._~-]) out="$out$c" ;;
      *) printf -v n '%d' "'$c"; printf -v h '%%%02X' $(( n & 255 )); out="$out$h" ;;
    esac
  done
  printf '%s' "$out"
}

# Sets __gilvt_hnum / __gilvt_htext to the number and text of the newest history entry ("" when there is
# none). Parameter expansion only: `[[ =~ ]]` would overwrite the user's BASH_REMATCH.
__gilvt_hist_entry() {
  __gilvt_hnum= __gilvt_htext=
  local entry rest
  entry=$(HISTTIMEFORMAT= builtin history 1 2>/dev/null)
  # `history` prints "%5d", then '*' (modified) or ' ', then ' ', then the line.
  rest=${entry#"${entry%%[![:space:]]*}"}
  __gilvt_hnum=${rest%%[!0-9]*}
  case "$__gilvt_hnum" in ''|*[!0-9]*) __gilvt_hnum=; return 0 ;; esac
  rest=${rest#"$__gilvt_hnum"}
  __gilvt_htext=${rest:2}
}

# The history number seen at the last command: a command that did not enter the history
# (HISTCONTROL=ignorespace, `set +o history`) leaves `history 1` on the previous entry. Set at the first
# prompt (unset until then): bash reads HISTFILE after the startup files, so it is not known here.
unset __gilvt_last_hist

# Sets __gilvt_line to the command line being run (from the history; cut to 2000 bytes so the OSC stays
# under gilvt's 8 KB limit even when every byte is escaped), or to "" when it was not recorded. $1 is
# $BASH_COMMAND at the DEBUG trap. A new history number means a recorded command. An unchanged number
# is either an unrecorded command (ignorespace) or a duplicate that ignoredups/erasedups did not
# renumber; the entry is reported only when it equals $1 (a repeated simple command). Anything else
# reports nothing: a repeated pipeline or list under ignoredups then shows as an unknown command, which
# is safer than reporting a stale line. Runs in the current shell (not in `$(...)`): __gilvt_last_hist
# must survive to the next command.
__gilvt_read_cmdline() {
  local LC_ALL=C
  __gilvt_line=
  __gilvt_hist_entry
  [ -n "$__gilvt_hnum" ] || return 0
  if [ "$__gilvt_hnum" = "${__gilvt_last_hist-}" ]; then
    [ -n "$1" ] && [ "$__gilvt_htext" = "$1" ] || return 0
  fi
  __gilvt_last_hist=$__gilvt_hnum
  __gilvt_line=${__gilvt_htext:0:2000}
}

__gilvt_at_prompt=
__gilvt_cmd_running=

__gilvt_prompt() {
  local ret=$?
  # Disarm while PROMPT_COMMAND runs so the user's prompt commands do not count as a command.
  __gilvt_at_prompt=
  if [ -n "$__gilvt_cmd_running" ]; then
    printf '\033]133;D;%s\033\\' "$ret"
    __gilvt_cmd_running=
  fi
  # Profile scripts (path_helper, brew shellenv) may reorder PATH: move GILVT_BIN_DIR back to the front.
  if [ -n "${GILVT_BIN_DIR-}" ]; then
    case "$PATH:" in
      "$GILVT_BIN_DIR:"*) ;;
      *)
        local p=":$PATH:"
        while case "$p" in *":$GILVT_BIN_DIR:"*) true ;; *) false ;; esac; do
          p="${p/":$GILVT_BIN_DIR:"/:}"
        done
        p="${p#:}"
        p="${p%:}"
        PATH="$GILVT_BIN_DIR${p:+:$p}"
        ;;
    esac
  fi
  printf '\033]7;file://%s%s\033\\' "$HOSTNAME" "$(__gilvt_urlencode "$PWD")"
  printf '\033]133;A\033\\'
  [ -n "$__gilvt_agents_checked" ] || __gilvt_define_agents
  if [ -z "${__gilvt_last_hist+set}" ]; then
    __gilvt_hist_entry
    __gilvt_last_hist=$__gilvt_hnum
  fi
  return $ret
}

# Agent wrappers: inside gilvt, `claude` / `codex` (and wrappers listed in GILVT_CODEX_COMMANDS) run with gilvt's hooks added by
# `gilvt hook <kind>-args` (NUL-separated argv; bash 3.2 compatible). Defined once, at the first
# prompt, only for commands that exist and that are not already an alias or function.
# GILVT_CLAUDE_COMMANDS / GILVT_CODEX_COMMANDS (space-separated) choose the names;
# GILVT_NO_AGENT_WRAPPERS=1 opts out. From your own alias or function, call
# `gilvt_agent claude|codex <command> [args…]`.
gilvt_agent() {
  local __gilvt_kind="$1" __gilvt_cmd="$2" __gilvt_a
  local -a __gilvt_argv
  shift 2
  if [ -n "${GILVT_SOCKET-}" ] && [ -n "${GILVT_BIN_DIR-}" ] && [ -x "$GILVT_BIN_DIR/gilvt" ]; then
    while IFS= read -r -d '' __gilvt_a; do
      __gilvt_argv[${#__gilvt_argv[@]}]="$__gilvt_a"
    done < <("$GILVT_BIN_DIR/gilvt" hook "$__gilvt_kind-args" -- "$@" 2>/dev/null)
  fi
  if [ ${#__gilvt_argv[@]} -gt 0 ]; then
    command "$__gilvt_cmd" "${__gilvt_argv[@]}"
  else
    command "$__gilvt_cmd" "$@"
  fi
}

__gilvt_agents_checked=
__gilvt_define_agents() {
  __gilvt_agents_checked=1
  [ -z "${GILVT_NO_AGENT_WRAPPERS-}" ] && [ -n "${GILVT_SOCKET-}" ] && [ -x "${GILVT_BIN_DIR-}/gilvt" ] || return 0
  local kind name names
  local -a list
  for kind in claude codex; do
    if [ "$kind" = claude ]; then names="${GILVT_CLAUDE_COMMANDS-claude}"; else names="${GILVT_CODEX_COMMANDS-codex}"; fi
    # read -a, not $names: no globbing of the list.
    IFS=' ' read -r -a list <<< "$names"
    for name in ${list[@]+"${list[@]}"}; do
      case "$name" in ''|*[!A-Za-z0-9._+-]*) continue ;; esac
      alias "$name" >/dev/null 2>&1 && continue
      declare -F "$name" >/dev/null 2>&1 && continue
      type -P "$name" >/dev/null 2>&1 || continue
      eval "function $name { gilvt_agent $kind $name \"\$@\"; }"
    done
  done
}

# Runs last in PROMPT_COMMAND: after any prompt theme rebuilt PS1, and re-arms preexec.
__gilvt_arm() {
  local ret=$?
  case "$PS1" in *'133;B'*) ;; *) PS1="$PS1"'\[\033]133;B\033\\\]' ;; esac
  __gilvt_at_prompt=1
  return $ret
}

__gilvt_preexec() {
  local __gilvt_bc="$BASH_COMMAND"  # the simple command about to run; read before anything else runs
  [ -n "$__gilvt_at_prompt" ] || return 0
  [ -n "${COMP_LINE-}" ] && return 0
  case "$BASH_COMMAND" in __gilvt_prompt*|__gilvt_arm*) return 0 ;; esac
  __gilvt_at_prompt=
  __gilvt_cmd_running=1
  __gilvt_read_cmdline "$__gilvt_bc"
  if [ -n "$__gilvt_line" ]; then
    printf '\033]133;C;cmdline_url=%s\033\\' "$(__gilvt_urlencode_fast "$__gilvt_line")"
  else
    printf '\033]133;C\033\\'
  fi
}

# Do not clobber a DEBUG trap the user already installed (command marks are then unavailable).
if [ -z "$(trap -p DEBUG)" ]; then
  trap '__gilvt_preexec' DEBUG
fi
# Newline-joined: the user's value may itself end in ';' or a newline.
PROMPT_COMMAND="__gilvt_prompt"$'\n'"${PROMPT_COMMAND-}"$'\n'"__gilvt_arm"
