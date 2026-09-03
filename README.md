# godot-nvm

`godot-nvm` is a terminal project dashboard for Godot projects whose editor is
provided by a Nix flake. It discovers default and named dev shells, verifies the
actual `godot` executable and version, and starts the editor detached from the
dashboard.

## Install

### Nix profile

```sh
nix profile install github:alexishachemi/godot-nvm
```

### NixOS configuration with flakes

Add `godot-nvm` as an input to your NixOS configuration flake and install its
default package. Making its `nixpkgs` input follow the system input avoids
evaluating and locking a second nixpkgs revision.

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    godot-nvm = {
      url = "github:alexishachemi/godot-nvm";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, godot-nvm, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        ({ pkgs, ... }: {
          environment.systemPackages = [
            godot-nvm.packages.${pkgs.system}.default
          ];

          # Optional: enables `gnvm` and lets open+close exit the terminal shell.
          programs.zsh.interactiveShellInit = ''
            eval "$(${godot-nvm.packages.${pkgs.system}.default}/bin/godot-nvm shell-init zsh)"
          '';
        })
      ];
    };
  };
}
```

Replace `my-host` and the system architecture as appropriate, then rebuild:

```sh
sudo nixos-rebuild switch --flake .#my-host
```

To update the dashboard later, update its locked input and rebuild the system:

```sh
nix flake update godot-nvm
sudo nixos-rebuild switch --flake .#my-host
```

For Bash, use `programs.bash.interactiveShellInit` and change the command's final
argument from `zsh` to `bash`.

### Shell integration

Run the dashboard directly with `godot-nvm`. For the action that launches Godot
and closes the invoking terminal shell, add this to your shell configuration:

```sh
# zsh
eval "$(godot-nvm shell-init zsh)"

# bash
eval "$(godot-nvm shell-init bash)"
```

Then launch the dashboard with `gnvm`. The normal open action leaves `gnvm`
running; the open-and-close action detaches Godot and exits the invoking shell.

## Dashboard

- `o` or `Enter`: open the selected project and keep the dashboard open.
- `x`: open the project and close the invoking shell when shell integration is active.
- `a`: add one project or scan every direct child of a directory.
- `n`: create a project, project flake, lock file, icon, and optional `.envrc`.
- `r`: reevaluate the selected project's Nix dev shell.
- `d`: unregister a project without deleting any project files.
- `,`: configure the default projects directory and direnv generation.
- `/`: filter projects by name or path.

Scanning recognizes Godot configuration files with multiline dictionaries,
arrays, and constructor expressions. A project only needs a structurally valid
`project.godot` with a positive top-level `config_version` to appear in scan
results.

Projects without `flake.nix` appear as **needs flake**. Adding one opens the
Godot version/tool form and, after confirmation, installs and validates a
generated `flake.nix` and `flake.lock` before registering the project.

When an existing flake has no dev shell for the current system, cannot be
evaluated, or does not expose a working `godot` command, the project is not
registered. The warning dialog defaults to cancelling that project. An explicit
overwrite choice generates a replacement only after another confirmation; the
original `flake.nix` and `flake.lock` are moved to a timestamped
`.godot-nvm-backup-*` directory. Existing `.envrc` files are always preserved.

## Generated project environments

The new-project and missing-flake workflows accept exact official stable Godot
tags such as `4.7.1-stable`. The official Linux editor archive is prefetched into
the Nix store, and its hash is embedded in a generated FHS-wrapped package. Extra
tools are accepted as nixpkgs attribute paths such as `git` or
`python3Packages.pygame`.

Generation requires network access the first time a release is used. Existing
registered projects remain usable offline when their Nix inputs and Godot archive
are already cached.

## Terminal images

Project PNG, JPEG, WebP, GIF, BMP, ICO, and SVG icons are rendered when Kitty,
Sixel, or iTerm2 graphics support is detected. Both `res://` icon paths and the
`uid://` references written by recent Godot versions are supported. Unsupported
terminals intentionally use a text-only layout; there is no Unicode image
approximation.

## State and logs

The registry follows XDG directories and is written atomically. Detached editor
output is stored below the XDG state directory, with the latest five logs retained
per project.
