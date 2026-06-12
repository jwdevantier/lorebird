

## Done
- JWZ threading algorithm (`./src/thread.rs`)

## Doing

## TODO
- Implement Lua VM integration (mlua)
    - call VM to read configuration
    - define initial configuration
        - list of profiles:
            - maildir location
            - maildir fetch hook (e.g. call `lorefetch`)
            -   fetch-hook should always get: profile name, maildir path
            - (optionally) list of views
                - a view is a named filter/search query -- will be an item in the GUI which, if clicked, filters the visible mail.
        - send hook
            - shall always receive 'profile name' (in case per-profile rules for sending applies)
        - reply-template hook
            - shall receive profile name and actual email
            - can be used to write out a reply template
        - pre-send hook (?)
            - investigate; could be used to sign mail for instance?
- Use mail-parse library and friends to actually parse a mail-dir
    - and then call the threading code
- Define query DSL parser
    - probably based on Xapian syntax
    - find a way to map to a sqlite3 query, see lorebird go demo
- Define sqlite3 integration
    - see https://youtu.be/eXMA_2dEMO0 -- define FTS5 tables, triggers for keeping in sync etc
    - would be ideal if index is maintained incrementally
    - index should be stored on disk inside the maildir (?)
- UI: Define base mail view
    - GTK: (sorted Vec<Thread>) -> gio::ListStore -> TreeListModel -> ColumnView
- UI: Define search bar
- UI: Define side-bar for viewing and switching between profiles
    - one entry for the profile itself, sub-entries for each defined view
- UI: Define button(s) to fetch & index new mail
- UI: Define ability to read mail (in-app? popup window?)
- UI: Define a mail writer/reply functionality
    - rely on Lua VM hook to send mail
