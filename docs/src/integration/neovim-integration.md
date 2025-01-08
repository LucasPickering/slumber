# Neovim Integration 

Slumber can be integrated into Neovim to allow you quickly switch between your codebase and slumber.

Ensure you have `which-key` and `toggleterm` installed. Most premade distros (LunarVim, etc) will have these installed by default. 

Add this snippet to your Neovim config:
```lua
local Slumber = {}
Slumber.toggle = function()
    local Terminal = require("toggleterm.terminal").Terminal
    local slumber = Terminal:new {
        cmd = "slumber",
        hidden = true,
        direction = "float",
        float_opts = {
            border = "none",
            width = 100000,
            height = 100000,
        },
        on_open = function(_)
            vim.cmd "startinsert!"
        end,
        on_close = function(_) end,
        count = 99,
    }
    slumber:toggle()
end

local wk = require("which-key")
wk.add({
    -- Map space V S to open slumber 
    { "<leader>vs", Slumber.toggle, desc = "Open in Slumber"}
})
```
