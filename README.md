<div align="center">
  <img src="crates/lorebird-gtk/resources/org.lorebird.app.256.png" alt="LoreBird" width="256">

# LoreBird
</div>

# Introduction
Do you wish to interact with the QEMU- or Linux Kernel mailing lists? Are you looking for
something a little easier to manage than a terminal mail reader (aerc, (neomutt), ...) with
custom scripts for fetching mail through lei, indexing and search using notmuch and
sending mail using a MTA like msmtp ?

If so, Lorebird might be for you.

This project has started in stages as I was fighting lei and later running out of
energy trying to manage notmuch and my mailreader.


Lorebird provides a graphical UI, shows mail organized by threads and is configured
via Lua, with hooks for events like:
- `on_fetch` (when the fetch mail button is clicked) 
- `on_reply` (when the compose/reply window is drawn, you can customize the mail before the user writes their message)
- `on_send` what to do when sending a mail

You can absolutely invoke lei and an MTA like msmtp if you want - or you can use
the built-in replacements through functions conveniently exposed to the Lua runtime.
Also - your mail is automatically indexed by Sqlite's FTS5 (full text search) plugin
and a Xapian-like query language allows you to quickly filter the threads shown as
you type your query.


# Installing

It is early days.

## Linux

The easiest approach is to use the flake development shell like so:

```
nix develop .#
```

Then you can compile the application as usual by running `cargo build --release -p lorebird`.

IF you want to build the application without Nix, then take a look at the flake.nix file to see
the packages used in the development shell, you can likely find similarly named packages in your
distribution. May the odds be ever in your favor.

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

You are very free to pollute your environment using Brew, or you can simply use the provided Nix development shell like so:

```
nix develop .#
```

Afterwards, you can compile the application as usual by running `cargo build --release -p lorebird`
