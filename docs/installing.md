# Installing
For now, support reflects how I personally test across the platforms.

## Linux

The easiest approach is to use the flake development shell like so:

```
nix develop .#
```

Then you can compile the application as usual by running `cargo build --release -p lorebird`.

IF you want to build the application without Nix, then take a look at the flake.nix file to see the packages used in the development shell, you can likely find similarly named packages in your distribution. May the odds be ever in your favor.

## Windows
The CI pipeline will produce bundled zips for each release. The steps below will only be required if you wish to compile the application yourself.

### Install MSYS2
```
winget install --id=MSYS2.MSYS2 -e --source winget
```

Open a `MSYS UCR64t` shell (search for it in the Windows menu), then install the required packages like so:

```
pacman -S mingw-w64-ucrt-x86_64-{rust,gcc,pkgconf,gtk4,gtksourceview5}
```

After this, you can compile the application as usual by running `cargo build --release -p lorebird`


## MacOS
The officially supported way is to use the provided Nix flake and its devshell or package.
```
nix develop .#
```

Afterwards, you can compile the application as usual by running `cargo build --release -p lorebird`

IF you want to build the application without Nix, then take a look at the flake.nix file to see the packages used in the development shell, you can likely find similarly named packages in brew.

