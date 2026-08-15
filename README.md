# Lush

This is my own custom Linux shell, Lush. I decided to try to combine the autocomplete of `fish` and the compatibility of `bash` into one shell.
This is also to go with my other project, [LumiTerm](https://github.com/ShadowBytess/LumiTerm) to work together, but LumiTerm is **NOT** required.

# Known Issues:
- No custom colours (Working on fixing it, please don't ask me how long it'll take)
- Aliases are buggy (Attempting to figure out why)

That's all the issues for now, at least until I find more.

# How to build:

install Cargo
```
sudo pacman -S cargo (Arch-based)
sudo dnf install cargo (Fedora-based)
sudo apt install cargo (Debian-based)
```
Then:
```
git clone https://github.com/ShadowBytess/Lush.git
cd lush
cargo build
cargo run
```

# This should boot you straight into Lush.
If it does not, please open an [Issue](https://github.com/ShadowBytess/Lush/issues) on this repository and give a detailed description of your issue, as well as the following:

- Any error messages it gives you
- Any build bugs you've had
- Any missing dependencies (**PLEASE** make sure you have all the dependencies)
- Anything you think is helpful

Failing to do the above will get your Issue ignored. Please do not make low-effort Issues. Alongside this, **PLEASE** look for your Issue before making a new one. You never know if someone might've had the same bug
as you and it might've been fixed.

Thanks for checking out the project.
