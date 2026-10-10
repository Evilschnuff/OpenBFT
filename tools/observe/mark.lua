-- tools/observe/mark.lua: marking a recording in mpv.
--
-- Started by `observe.py mark`, which passes a JSON config (the markset's
-- keys, the marks CSV, the recording's frame rate) as script-opt
-- observe-config. Each markset key appends one row (frame, time, label,
-- note) to the CSV; rows are only ever appended. Step frames with mpv's own
-- keys (. and ,).
--
-- Built in: BS voids the last row (by appending a void row), ! marks
-- something as not observable, TAB / Shift+TAB jump between candidate
-- times, F1 shows the keys.

local mp = require "mp"
local utils = require "mp.utils"
local options = require "mp.options"
local has_input, input = pcall(require, "mp.input")

local opts = { config = "" }
options.read_options(opts, "observe")
local fh = io.open(opts.config, "r")
if not fh then
    mp.msg.error("observe: cannot read config " .. opts.config)
    return
end
local cfg = utils.parse_json(fh:read("*a"))
fh:close()

local fps = cfg.fps_num / cfg.fps_den
local rows = cfg.rows -- data rows in the CSV so far
local live = cfg.live -- rows that are neither void nor voided, oldest first

local function frame_of(t)
    return math.floor((t - cfg.video_offset) * fps + 0.5)
end

local function now()
    local t = mp.get_property_number("time-pos")
    if t == nil then
        mp.osd_message("observe: no position yet", 2)
        return nil
    end
    return frame_of(t), t
end

local function append(frame, t, label, note)
    local out = io.open(cfg.csv, "a")
    if not out then
        mp.osd_message("observe: cannot write " .. cfg.csv, 5)
        return nil
    end
    local clean = note:gsub("[\r\n]", " ")
    local quoted = clean:gsub('"', '""')
    out:write(string.format('%d,%.6f,%s,"%s"\n', frame, t, label, quoted))
    out:close()
    rows = rows + 1
    return rows
end

-- The mouse position in the recording's pixels.
local function video_pos()
    local m = mp.get_property_native("mouse-pos")
    local d = mp.get_property_native("osd-dimensions")
    local vw = mp.get_property_number("video-params/w")
    local vh = mp.get_property_number("video-params/h")
    if not (m and d and vw and vh) then
        return nil
    end
    local sw = d.w - d.ml - d.mr
    local sh = d.h - d.mt - d.mb
    if sw <= 0 or sh <= 0 then
        return nil
    end
    return (m.x - d.ml) * vw / sw, (m.y - d.mt) * vh / sh
end

local function mark(k)
    local frame, t = now()
    if not frame then
        return
    end
    local parts = {}
    if k.pos then
        local x, y = video_pos()
        if not x then
            mp.osd_message("observe: no mouse position; nothing marked", 3)
            return
        end
        parts[#parts + 1] = string.format("x=%.1f y=%.1f", x, y)
    end
    if k.grab then
        local name = string.format("%s-f%06d-%s.png", cfg.rec_stem, frame, k.label)
        mp.commandv("screenshot-to-file", utils.join_path(cfg.grabs_dir, name), "video")
        parts[#parts + 1] = "grab=" .. name
    end
    local function finish(text)
        if text and text ~= "" then
            table.insert(parts, 1, text)
        end
        local row = append(frame, t, k.label, table.concat(parts, "; "))
        if row then
            if k.label ~= "void" then
                live[#live + 1] = row
            end
            mp.osd_message(string.format("row %d: %s at frame %d", row, k.label, frame), 3)
        end
    end
    if not k.note then
        finish("")
    elseif not has_input then
        finish("")
        mp.osd_message("observe: this mpv has no mp.input; the note is empty", 3)
    else
        local submitted = false
        input.get({
            prompt = k.label .. " note: ",
            submit = function(text)
                submitted = true
                input.terminate()
                finish(text)
            end,
            closed = function()
                if not submitted then
                    mp.osd_message("observe: cancelled, nothing marked", 2)
                end
            end,
        })
    end
end

local function void()
    local target = table.remove(live)
    if not target then
        mp.osd_message("observe: nothing to void", 2)
        return
    end
    local frame, t = now()
    append(frame or 0, t or 0, "void", "row " .. target)
    mp.osd_message("observe: voided row " .. target, 3)
end

local function jump(dir)
    local t = mp.get_property_number("time-pos") or 0
    local target, index
    if dir > 0 then
        for i, c in ipairs(cfg.candidates) do
            if c > t + 1e-3 then
                target, index = c, i
                break
            end
        end
    else
        for i = #cfg.candidates, 1, -1 do
            if cfg.candidates[i] < t - 1e-3 then
                target, index = cfg.candidates[i], i
                break
            end
        end
    end
    if not target then
        mp.osd_message("observe: no further candidate", 2)
        return
    end
    mp.set_property_bool("pause", true)
    mp.commandv("seek", tostring(target), "absolute+exact")
    mp.osd_message(string.format("candidate %d/%d", index, #cfg.candidates), 2)
end

local function help()
    local lines = { "markset " .. cfg.markset .. " -> " .. cfg.csv, "" }
    for _, k in ipairs(cfg.keys) do
        lines[#lines + 1] = string.format("%-6s %-16s %s", k.key, k.label, k.help or "")
    end
    lines[#lines + 1] = ""
    lines[#lines + 1] = "BS void last row   TAB/Shift+TAB candidates   . , step frames   SPACE play"
    mp.osd_message(table.concat(lines, "\n"), 15)
end

for _, k in ipairs(cfg.keys) do
    mp.add_forced_key_binding(k.key, "observe-" .. k.label, function() mark(k) end)
end
mp.add_forced_key_binding("BS", "observe-void", void)
mp.add_forced_key_binding("TAB", "observe-next", function() jump(1) end)
mp.add_forced_key_binding("Shift+TAB", "observe-prev", function() jump(-1) end)
mp.add_forced_key_binding("F1", "observe-help", help)

mp.observe_property("time-pos", "number", function(_, t)
    if t then
        mp.set_property("osd-msg1", string.format("frame %d   %.3f s   [%s, F1 keys]", frame_of(t), t, cfg.markset))
    end
end)
mp.register_event("file-loaded", function()
    mp.osd_message("observe: " .. cfg.markset .. " (F1 shows the keys)", 4)
end)
