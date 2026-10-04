__sv_zdotdir="$ZDOTDIR"; ZDOTDIR="$SHELLVIEW_USER_ZDOTDIR"
[[ -r "$ZDOTDIR/.zshrc" ]] && source "$ZDOTDIR/.zshrc"
SHELLVIEW_USER_ZDOTDIR="$ZDOTDIR"; ZDOTDIR="$__sv_zdotdir"

# ---- ShellView hooks: cwd, command text, start/finish + exit code
autoload -Uz add-zsh-hook
__sv_osc() { printf '\e]%s\a' "$1" }
__sv_running=0
__sv_precmd() {
  local ec=$?
  (( __sv_running )) && __sv_osc "133;D;$ec"
  __sv_running=0
  __sv_osc "7;file://${HOST}${PWD}"
  __sv_osc "133;A"
}
__sv_preexec() {
  __sv_running=1
  __sv_osc "7770;cmd;$(print -rn -- "$1" | base64 | tr -d '\n')"
  __sv_osc "133;C"
}
add-zsh-hook precmd __sv_precmd
add-zsh-hook preexec __sv_preexec

# non-login shells never read .zlogin: hand ZDOTDIR back to the user now
[[ -o login ]] || { ZDOTDIR="$SHELLVIEW_USER_ZDOTDIR"; unset SHELLVIEW_USER_ZDOTDIR SHELLVIEW_ORIG_ZDOTDIR; }
