-- `/reasoning` — switch reasoning effort. Direct with an arg; picker without.

local function build_items()
  local out = {}
  for _, effort in ipairs(smelt.reasoning.cycle_list() or {}) do
    out[#out + 1] = {
      label        = effort,
      _effort      = effort,
    }
  end
  return out
end

local cycle_list = smelt.reasoning.cycle_list() or {}
local effort_labels = {}
for _, e in ipairs(cycle_list) do
  effort_labels[#effort_labels + 1] = e
end

smelt.cmd.picker("reasoning", {
  desc     = "switch reasoning effort",
  args     = effort_labels,
  items    = build_items,
  apply    = function(arg) smelt.reasoning(arg) end,
  prepare  = function()
    if #cycle_list == 0 then smelt.notify.error("no reasoning cycle configured") end
  end,
  on_enter = function(item) if item._effort then smelt.cmd.run("/reasoning " .. item._effort) end end,
})
