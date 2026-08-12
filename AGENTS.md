# Repository Instructions

- After every repository change, rebuild and install `displayctl` in the user executable directory:

  ```sh
  swiftc main.swift -o displayctl
  install -m 755 displayctl "$HOME/.local/bin/displayctl"
  ```

  `$HOME/.local/bin` is on the user's Bash `PATH`.
- After every repository change, commit the change and push the commit to the current remote branch.
