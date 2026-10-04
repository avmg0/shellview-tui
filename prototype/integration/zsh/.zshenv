# ShellView: forward to the user's real .zshenv
__sv_zdotdir="$ZDOTDIR"
ZDOTDIR="${SHELLVIEW_ORIG_ZDOTDIR:-$HOME}"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
SHELLVIEW_USER_ZDOTDIR="$ZDOTDIR"
ZDOTDIR="$__sv_zdotdir"
