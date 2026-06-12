
add limit:<num> support in the query lang
    currently: ParsedQuery default limit is 50, UI overrides to 5000
    (see app_state.rs rebuild_thread_tree_searched)
    idea: let user type e.g. "from:alice limit:10" to cap results



keybinds operating on the current mail // right-click menu for other mail
    Reply (hard-coded)
    Show all headers
    <user-defined actions>
        Such as extract message-id (get the mail object, do whatever)
        Such as pass to b4 shazam for download.





Maybe rewrite and integrate lorefetch into tool

(Probably) offer a built-in mail sender; maybe as a toggleable FEATURE


Set up CI for MacOS and Windows

Write (tested) build instructions for Mac, Linux (Debian), NixOS, Windows


## Done
Make DPI user-configurable

Simplify - always syntax highlight as diff

add a progress bar somewhere when indexing
    especially if profile changes trigger indexing (as I think it will?)
