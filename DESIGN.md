
A mail reader

# Problem
Reading mailing lists like those indexed at lore.kernel.org is often handled by two complementary programs:
- something to fetch the mail into a mailbox or maildir
- something to READ the mail - a mail *reader*

I have implemented `lorefetch` to fetch mail - replacing the brittle `lei` script.

However, reading the mail still requires something like 
- a minimal IMAP/POP server wrapper over a maildir -- (for Thunderbird and friends)
- a classic mail reader like mutt, aerc or mu4e.


I don't like the mail readers on offer. Mutt and Aerc are "almost vim", but not quite - and often require integrating `notmuch` to give you decent filtering/searching - another component which can be configured wrong.

# Design Summary

## Graphical
Reading mailing lists is an infrequent task, I will forget the various keybinds. Hence I want a *graphical* tool, ideally keybind-friendly, instead. So I'll write a cross-platform, *graphical* tool using GTK.

## Lua for scripting hooks and configuration
Also, I want flexibility without an overly complicated configuration file format or a lot of code - so I will integrate a Lua VM and express configuration in Lua - just as neovim would.
Similarly still to neovim, I will expose relevant APIs to the Lua VM to make it easy to extend the application in certain ways such as installing reply hooks (to generate an initial mail) and so on. 
This provides flexibility without too many lines of code.

## Mail fetching is a configurable, external call
Fetching mail should be a hook that can be implemented from Lua. This should allow you to spawn some other process (like Lorefetch or lei).

NOTE: I will only support lorefetch directly - lorefetch has a nicer approach to naming mail files in a stable manner.

## Mail indexing is a manually triggered act
Mail indexing *may* take a while, in case of having large quantities of mail, seeing as we will employ the JWZ threading algorithm to build the whole index anew.

I think this will be triggered automatically after fetching mail IFF we know new mail was fetched.
(maybe just compare number of files in maildir before and after).

If possible, show progress bar.

## Mail search/filtering
We will rely on a Sqlite3 database with full-text-search support. We also will devise a querying language (likely xapian-style) allowing you to search in specific fields of the mail and to string together criteria.

I tend to think we will always include any thread with one or more items of mail which matches the query. Maybe greying out all mail entries which did not match.

The GUI shall have a search bar above the messages list which can be used.
The config shall be able to add "views" - essentially named, stored queries which, if clicked, are applied to filter the mail

## Writing mails - basic support
I am hoping a gtk source view window, defaulting to populated `References:`, `In-Reply-To:`, `Subject:`, `To:`, `Cc:` and `Bcc:` headers will suffice.

As a stretch-goal, support limited code/diff highlighting.

Finally, the actual SENDING of an email is, as for aerc/mutt/mu4e handled outside this program, we will deliver the mail to a sender and rely on it to deliver the mail.

## Rust
This I struggled with. But fact is Rust has excellent GTK bindings (and a book on the subject!) and a strong ecosystem with good libraries for parsing mail (just as Go does, btw).
