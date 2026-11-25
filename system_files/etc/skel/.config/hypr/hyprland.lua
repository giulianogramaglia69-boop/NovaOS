-- NovaOS starter config for Hyprland.
-- Lua format (Hyprland 0.55+). Docs: https://wiki.hypr.land/Configuring/Start/
-- hyprlock / hypridle / hyprpaper still use the old hyprlang (.conf) format.
-- Dux Shell (bar, launcher, control center, notifications, lock screen) is
-- started below with `dms run`. Run `dms setup` once after first login for
-- Dux's own configuration.

local mod = "SUPER"
local terminal = "alacritty"

hl.env("XCURSOR_SIZE", "24")

hl.on("hyprland.start", function()
    hl.exec_cmd("dms run")
    hl.exec_cmd("waybar")
    -- NovaOS wallpaper (hyprpaper.conf in the same directory).
    hl.exec_cmd("hyprpaper")
end)

hl.config({
    general = {
        gaps_in = 5,
        gaps_out = 10,
        border_size = 2,
        layout = "dwindle",
    },
    decoration = {
        rounding = 8,
    },
    input = {
        kb_layout = "it",
        follow_mouse = 1,
    },
})

hl.bind(mod .. " + Return", hl.dsp.exec_cmd(terminal))
hl.bind(mod .. " + D", hl.dsp.exec_cmd("wofi --show drun"))
hl.bind(mod .. " + Q", hl.dsp.window.close())
hl.bind(mod .. " + F", hl.dsp.window.fullscreen())
hl.bind(mod .. " + V", hl.dsp.window.float({ action = "toggle" }))
hl.bind(mod .. " + SHIFT + E", hl.dsp.exit())

for dir, key in pairs({ left = "H", right = "L", up = "K", down = "J" }) do
    hl.bind(mod .. " + " .. key, hl.dsp.focus({ direction = dir }))
end

for i = 1, 10 do
    local key = i % 10 -- workspace 10 is on the 0 key
    hl.bind(mod .. " + " .. key, hl.dsp.focus({ workspace = i }))
    hl.bind(mod .. " + SHIFT + " .. key, hl.dsp.window.move({ workspace = i }))
end

hl.bind(mod .. " + mouse:272", hl.dsp.window.drag(), { mouse = true })
hl.bind(mod .. " + mouse:273", hl.dsp.window.resize(), { mouse = true })
