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

# Further Documentation

- [Building/Installing LoreBird](./docs/installing.md)
- [Configuring LoreBird](./docs/config.md)
- [The Lua API](./docs/lua_api.md)

# License

This project is licensed under the [MIT License](LICENSE).
