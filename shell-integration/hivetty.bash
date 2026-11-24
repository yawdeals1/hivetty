# Hivetty shell integration for bash (Git Bash / MSYS2 / WSL).
# Loaded via --init-file, so emulate login-shell startup first.
if [ -z "$__hivetty_loaded" ]; then
  __hivetty_loaded=1
  [ -r /etc/profile ] && . /etc/profile
  if [ -r ~/.bash_profile ]; then . ~/.bash_profile
  elif [ -r ~/.bash_login ]; then . ~/.bash_login
  elif [ -r ~/.profile ]; then . ~/.profile
  elif [ -r ~/.bashrc ]; then . ~/.bashrc
  fi

  __hivetty_ec=0
  __hivetty_last_hist=$(HISTTIMEFORMAT= builtin history 1 | awk '{print $1}')

  __hivetty_pre() { __hivetty_ec=$?; }
  __hivetty_post() {
    local h
    h=$(HISTTIMEFORMAT= builtin history 1 | awk '{print $1}')
    if [ "$h" != "$__hivetty_last_hist" ]; then
      printf '\e]133;D;%s\a' "$__hivetty_ec"
      __hivetty_last_hist=$h
    fi
    local p=${PWD//%/%25}; p=${p// /%20}
    printf '\e]7;file://%s%s\a' "${HOSTNAME:-localhost}" "$p"
    case "$PS1" in
      *'133;A'*) ;;
      *) PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]' ;;
    esac
  }
  PS0=$'\e]133;C\a'
  PROMPT_COMMAND="__hivetty_pre${PROMPT_COMMAND:+;$PROMPT_COMMAND};__hivetty_post"
fi
