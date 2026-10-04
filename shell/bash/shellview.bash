# ShellView shell integration for bash (works with macOS's bash 3.2 and bash 4/5).
# Loaded via `bash --rcfile <this> -i`. Sources your normal startup files first,
# then reports cwd / command start / exit code to ShellView with OSC escapes.

if [ -n "$SHELLVIEW_LOGIN" ]; then
  [ -r /etc/profile ] && . /etc/profile
  if   [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
  elif [ -r "$HOME/.bash_login" ];   then . "$HOME/.bash_login"
  elif [ -r "$HOME/.profile" ];      then . "$HOME/.profile"
  fi
else
  [ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"
fi
unset SHELLVIEW_LOGIN

__sv_osc() { printf '\033]%s\007' "$1"; }
__sv_b64() { printf '%s' "$1" | base64 | tr -d '\n'; }
__sv_at_prompt=0
__sv_running=0

__sv_preexec() {
  [ "$__sv_at_prompt" = 1 ] || return 0
  case "$BASH_COMMAND" in __sv_precmd*) return 0 ;; esac
  [ -n "$COMP_LINE" ] && return 0
  __sv_at_prompt=0
  __sv_running=1
  local cmd
  cmd=$(HISTTIMEFORMAT= builtin history 1 | sed 's/^ *[0-9]*[* ]*//')
  __sv_osc "7770;cmd;$(__sv_b64 "$cmd")"
  __sv_osc "133;C"
}

__sv_precmd() {
  local ec=$?
  if [ "$__sv_running" = 1 ]; then __sv_osc "133;D;$ec"; fi
  __sv_running=0
  __sv_osc "7;file://${HOSTNAME}${PWD}"
  __sv_osc "133;A"
  __sv_at_prompt=1
  return $ec
}

case "$PROMPT_COMMAND" in
  *__sv_precmd*) ;;
  *) PROMPT_COMMAND="__sv_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}" ;;
esac
trap '__sv_preexec' DEBUG
