__sv_zdotdir="$ZDOTDIR"; ZDOTDIR="$SHELLVIEW_USER_ZDOTDIR"
[[ -r "$ZDOTDIR/.zprofile" ]] && source "$ZDOTDIR/.zprofile"
SHELLVIEW_USER_ZDOTDIR="$ZDOTDIR"; ZDOTDIR="$__sv_zdotdir"
