# gilvt hooks for fish, loaded from vendor_conf.d via XDG_DATA_DIRS.
status is-interactive; or exit

function __gilvt_prompt --on-event fish_prompt
    set -l ret $status
    if set -q __gilvt_cmd_running
        printf '\e]133;D;%s\e\\' $ret
        set -e __gilvt_cmd_running
    end
    # Profile scripts may reorder PATH: move GILVT_BIN_DIR back to the front.
    if set -q GILVT_BIN_DIR; and test "$PATH[1]" != "$GILVT_BIN_DIR"
        while set -l i (contains -i -- $GILVT_BIN_DIR $PATH)
            set -e PATH[$i]
        end
        set -gx PATH $GILVT_BIN_DIR $PATH
    end
    printf '\e]7;file://%s%s\e\\' (hostname) (string escape --style=url -- $PWD)
    printf '\e]133;A\e\\'
end

function __gilvt_preexec --on-event fish_preexec
    set -g __gilvt_cmd_running 1
    # $argv[1] is the command line; `string escape --style=url` escapes `;` and non-ASCII. `string sub`
    # counts characters: 600 of them are at most 2400 bytes, so the OSC stays under gilvt's 8 KB limit.
    # `string collect` keeps a multi-line command one argument (one C mark).
    printf '\e]133;C;cmdline_url=%s\e\\' (string escape --style=url -- (string sub -l 600 -- $argv[1] | string collect))
end

# Agent wrappers: inside gilvt, `claude` / `codex` (and wrappers listed in GILVT_CODEX_COMMANDS) run with gilvt's hooks added by
# `gilvt hook <kind>-args` (NUL-separated argv). Defined once, at the first prompt (after
# config.fish), only for commands that exist and that are not already a function or abbreviation.
# GILVT_CLAUDE_COMMANDS / GILVT_CODEX_COMMANDS (space-separated) choose the names;
# GILVT_NO_AGENT_WRAPPERS=1 opts out. From your own function, call
# `gilvt_agent claude|codex <command> [args…]`.
function gilvt_agent --description 'gilvt_agent claude|codex <command> [args…]: run an agent with gilvt hooks'
    set -l kind $argv[1]
    set -l cmd $argv[2]
    set -e argv[1]
    set -e argv[1]
    set -l bin "$GILVT_BIN_DIR/gilvt"
    if test -n "$GILVT_SOCKET"; and test -n "$GILVT_BIN_DIR"; and test -x "$bin"
        set -l full ("$bin" hook $kind-args -- $argv 2>/dev/null | string split0)
        if test (count $full) -gt 0
            command $cmd $full
            return $status
        end
    end
    command $cmd $argv
end

function __gilvt_define_agents --on-event fish_prompt
    functions -e __gilvt_define_agents
    test -z "$GILVT_NO_AGENT_WRAPPERS"; and test -n "$GILVT_SOCKET"; and test -x "$GILVT_BIN_DIR/gilvt"; or return 0
    for kind in claude codex
        set -l names claude
        set -q GILVT_CLAUDE_COMMANDS; and set names (string split -n ' ' -- $GILVT_CLAUDE_COMMANDS)
        if test $kind = codex
            set names codex
            set -q GILVT_CODEX_COMMANDS; and set names (string split -n ' ' -- $GILVT_CODEX_COMMANDS)
        end
        for name in $names
            string match -qr '^[A-Za-z0-9._+-]+$' -- $name; or continue
            functions -q -- $name; and continue
            abbr -q -- $name 2>/dev/null; and continue
            command -s -- $name >/dev/null; or continue
            function $name -V kind -V name --wraps $name
                gilvt_agent $kind $name $argv
            end
        end
    end
end
