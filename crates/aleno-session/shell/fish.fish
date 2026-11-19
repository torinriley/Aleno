# Aleno shell integration (fish): command blocks and cwd reporting.
if status is-interactive; and test "$ALENO_SHELL_INTEGRATION" != 0
    function __aleno_prompt --on-event fish_prompt
        printf '\e]7;file://%s%s\a\e]133;A\a' (hostname) (string escape --style=url -- $PWD | string replace -a '%2F' '/')
    end
    function __aleno_preexec --on-event fish_preexec
        printf '\e]133;C;cmd=%s\a' (string replace -ra '[[:cntrl:]]' ' ' -- $argv[1] | string sub -l 512)
    end
    function __aleno_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
end
