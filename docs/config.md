# Configuring

When first starting the application, you will see an empty sidebar with a message telling
you where to place a config file. It varies by platform, so do check.

## Overview
```lua
-- linux
local maildir = "/home/jwd/lorebird"
-- windows
-- local maildir = "C:\\Users\\jwd\\loreread\\INBOX"

local smtp = {
  host = "smtp.fastmail.com",
  -- 587 -> assume STARTTLS
  -- 465 -> assume SMTPS
  port = 587,
  -- normally determined by port (see above), but can be manually set
  -- starttls = false
  username = "jwd@defmacro.it",
  password = "<not gonna happen>",
}

config = {
  -- may be "light" (default) or "dark"
  theme = "dark",
  -- Scale the UI, 1.0 is the default.
  ui_scale = 1.3,

  user = {
    name = "Jesper Wendel Devantier",
    -- shown in `From` field of email replies
    email = "foss@defmacro.it",
  },

  profiles = {
    -- you may define as many profiles as you want
    ["mail I care about"] = {
      maildir = maildir,
      smtp = smtp,
      on_fetch = function(profile, maildir)
        -- what to do when fetching new mail
        return true
      end,
      views = {
        -- use this to define frequently used searches to quickly filter your mail
        -- (or manually type them into the search bar each time like a cave man)
        { label = "last week", query = "date:1w.." },
      },
    },
  },

  on_reply = function(profile, parent, mail)
    -- anything we want to do before the compose/reply window is shown
  end,

  on_send = function(profile, mail)
    -- what to do when clicking the 'send' button
  end,
}
```

## Fetching Mail

### Using in-app functionality
```lua
-- provie a Xapian search query, just as you would on lore.kernel.org or when using lei.
local query = [[l:qemu-devel AND (dfn:include/block/nvme.h OR dfn:hw/nvme/*) AND rt:6.month.ago..now]]
local result = lorefetch(maildir, query)
if result.ok then
  print(string.format([[%s: received %d new of %d total]], profile, result.new, result.count))
else
  print(string.format([[%s: fetch error: %s]], profile, result.error))
end
return result.ok
```

### Shelling out to an external program
```lua
-- apologies, I have not used lei in a while, but you get the point,
-- each separate argument is a string entry in an array
sh({
  "lei",
  "-I", "https://lore.kernel.org/all",
  "-t", 
  "l:qemu-devel AND (dfn:include/block/nvme.h OR dfn:hw/nvme/*) AND rt:6.month.ago"
})
```

## Sending Mail

### Using in-app functionality
```lua
local mail_str = mail_to_rfc2822(mail)
send_smtp(mail_str)
```

**TIP**: If you want to debug or otherwise see what gets sent, simply
try printing out the mail as-is or save it to a file:
```lua
local mail_str = mail_to_rfc2822(mail)
print(string.format("mail:\n---\n%s\n---", mail_str))
```

### Shelling out to an external program

```lua
sh({
  "lei",
  "-I", "https://lore.kernel.org/all",
  "-t", 
  "l:qemu-devel AND (dfn:include/block/nvme.h OR dfn:hw/nvme/*) AND rt:6.month.ago"
})
```

