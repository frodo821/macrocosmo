-- Lua UI DSL shadow definitions for existing Rust UI.
--
-- One fragment is currently wired into the ESC "Lua UI" preview tab. The rest
-- pressure-test the DSL authoring shape using the current primitive set:
-- section / vstack / hstack / grid / row / text / progress / button / action.

local ui = require("macrocosmo.ui")

local function stack(children)
    return ui.vstack { gap = "sm", children = children }
end

local function row(children, opts)
    opts = opts or {}
    return ui.hstack {
        gap = "sm",
        align_items = opts.align_items or "start",
        justify_content = opts.justify_content or "start",
        children = children,
    }
end

local function kv(key, value)
    return ui.hstack {
        gap = "md",
        children = {
            ui.text(key),
            ui.text(value),
        },
    }
end

local function action(label, command, opts)
    opts = opts or {}
    opts.label = label
    opts.command = command
    return ui.button(opts)
end

local function note(text)
    return ui.text("TODO: " .. text)
end

local function value_or(value, fallback)
    if value == nil then
        return fallback
    end
    return value
end

local function event_node(event, depth)
    local children = {}
    local prefix = string.rep("  ", depth)
    local row_children = {
        ui.text(prefix .. value_or(event.label, "")),
    }
    if event.progress ~= nil then
        table.insert(row_children, ui.progress(event.progress))
    end
    if event.eta ~= nil then
        table.insert(row_children, ui.text("ETA " .. tostring(event.eta)))
    end
    table.insert(children, row(row_children))

    local nested = value_or(event.children, {})
    for _, child in ipairs(nested) do
        table.insert(children, event_node(child, depth + 1))
    end
    return stack(children)
end

local function event_tree(view, empty_label)
    local children = {}
    local events = value_or(view.events, {})
    if #events == 0 then
        table.insert(children, ui.text(value_or(empty_label, "(nothing ongoing)")))
    else
        for _, event in ipairs(events) do
            table.insert(children, event_node(event, 0))
        end
    end
    return stack(children)
end

local function fragment(id, labels, context, title, body)
    return define_ui_fragment {
        id = id,
        labels = labels,
        context = context or {},
        render = function(view)
            return ui.section {
                title = title,
                children = {
                    body(view),
                },
            }
        end,
    }
end

local function bare_fragment(id, labels, context, tags, body)
    if body == nil then
        body = tags
        tags = nil
    end
    return define_ui_fragment {
        id = id,
        labels = labels,
        tags = tags or {},
        context = context or {},
        render = body,
    }
end

-- Frame chrome ---------------------------------------------------------------

bare_fragment(
    "core.ui.top_bar",
    { "chrome", "top_bar", "global", "time", "resources", "action_entrypoints" },
    { requires = { "empire" }, optional = { "clock", "observer_view" } },
    function(view)
        local function open_label(label, is_open)
            if is_open then
                return label .. " [open]"
            end
            return label
        end

        local resource_children = {}
        for _, resource in ipairs(value_or(view.resources, {})) do
            table.insert(
                resource_children,
                ui.text(value_or(resource.label, "?") .. ":" .. value_or(resource.stockpile, "0") .. " (" .. value_or(resource.net, "0") .. ")")
            )
        end

        local children = {
            ui.text(value_or(view.date, "Year 0 Month 0 Hexadies 0")),
            action("Pause", "time.pause"),
            action("Play", "time.play"),
            action("Fast", "time.fast"),
            ui.text(value_or(view.speed, "PAUSED")),
        }
        for _, resource_node in ipairs(resource_children) do
            table.insert(children, resource_node)
        end
        table.insert(children, action(open_label("Research", view.research_open), "ui.toggle.research"))
        table.insert(children, action(open_label("Diplomacy", view.diplomacy_open), "ui.toggle.diplomacy"))
        table.insert(children, action(open_label("Designer", view.ship_designer_open), "ui.toggle.ship_designer"))

        if view.observer_enabled then
            local label = "Observer Mode"
            if view.observer_read_only then
                label = label .. " [read-only]"
            end
            table.insert(children, ui.text(label))
        end

        return row(children, { align_items = "center" })
    end
)

bare_fragment(
    "core.ui.bottom_bar",
    { "chrome", "bottom_bar", "event_log" },
    { optional = { "event_log" } },
    function(view)
        local children = {
            ui.text("Event Log"),
        }
        local entries = value_or(view.entries, {})
        if #entries == 0 then
            table.insert(children, ui.text("No events yet."))
        else
            for _, entry in ipairs(entries) do
                table.insert(children, ui.text(value_or(entry.text, "")))
            end
        end
        return stack(children)
    end
)

fragment(
    "core.ui.notification_pills",
    { "overlay", "notifications", "transient" },
    { requires = { "empire" }, optional = { "notification_queue" } },
    "Notification Pills",
    function(view)
        local children = {}
        for _, notification in ipairs(value_or(view.notifications, {})) do
            local label = value_or(notification.glyph, "i") .. " " .. value_or(notification.title, "")
            local tooltip_children = {
                ui.text(value_or(notification.title, "")),
                ui.text(value_or(notification.description, "")),
            }
            if notification.remaining_hexadies ~= nil then
                table.insert(tooltip_children, ui.text("auto-dismiss in " .. tostring(notification.remaining_hexadies) .. " hex"))
            end

            local row_children = {
                ui.tooltip {
                    content = ui.text(label),
                    tooltip = tooltip_children,
                },
                action("Dismiss", "notification.dismiss:" .. tostring(notification.id)),
            }
            if notification.has_target then
                table.insert(row_children, action("Jump", "notification.jump:" .. tostring(notification.id)))
            end
            table.insert(children, row(row_children))
        end
        return stack(children)
    end
)

-- Navigation / selection -----------------------------------------------------

bare_fragment(
    "core.ui.outline",
    { "side_panel", "outline", "navigation", "systems", "ships" },
    { requires = { "empire" }, optional = { "selected_system", "selected_ship", "knowledge" } },
    function(view)
        local system_children = {}
        local systems = value_or(view.systems, {})
        if #systems == 0 then
            table.insert(system_children, ui.text("(no colonies)"))
        else
            for _, system in ipairs(systems) do
                local label = value_or(system.name, "Unknown")
                if system.is_capital then
                    label = label .. " [capital]"
                end
                if system.selected then
                    label = "> " .. label
                end
                local opts = { full_width = true }
                if view.selected_ship_active then
                    opts.secondary_command = "outline.command_system:" .. tostring(system.id)
                    opts.secondary_shift_command = "outline.command_system_default:" .. tostring(system.id)
                end
                table.insert(system_children, action(label, "outline.select_system:" .. tostring(system.id), opts))
            end
        end

        local transit_children = {}
        local in_transit = value_or(view.in_transit, {})
        if #in_transit == 0 then
            table.insert(transit_children, ui.text("(none)"))
        else
            for _, ship in ipairs(in_transit) do
                table.insert(transit_children, action(value_or(ship.name, "Ship") .. " [" .. value_or(ship.status, "?") .. "]", "outline.select_ship:" .. tostring(ship.id), { full_width = true }))
            end
        end

        return stack {
            ui.section {
                title = "Systems",
                children = {
                    ui.grid {
                        columns = 1,
                        children = system_children,
                    },
                },
            },
            ui.section {
                title = "In Transit",
                children = {
                    ui.grid {
                        columns = 1,
                        children = transit_children,
                    },
                },
            },
        }
    end
)

bare_fragment(
    "core.ui.context_menu.ship_move_to_system",
    { "modal", "context_menu", "ship", "commands", "action_heavy" },
    { requires = { "ship", "target_system" }, optional = { "empire", "target_planet" } },
    {
        part_of = "context_menu",
        target = "entity:system",
        ["ctx:selected"] = "entity:ship",
        command = "ship.move",
    },
    function(_)
        return stack {
            action("Move", "ship.move"),
        }
    end
)

bare_fragment(
    "core.ui.context_menu.ship_survey_system",
    { "modal", "context_menu", "ship", "commands", "survey", "action_heavy" },
    { requires = { "ship", "target_system" }, optional = { "empire" } },
    {
        part_of = "context_menu",
        target = "entity:system",
        ["ctx:selected"] = "entity:ship",
        ["ctx:selected:ship:class"] = "surveyor",
        command = "ship.survey",
    },
    function(_)
        return stack {
            action("Survey", "ship.survey"),
        }
    end
)

bare_fragment(
    "core.ui.context_menu.ship_colonize_system",
    { "modal", "context_menu", "ship", "commands", "colonize", "action_heavy" },
    { requires = { "ship", "target_system" }, optional = { "empire", "target_planet" } },
    {
        part_of = "context_menu",
        target = "entity:system",
        ["ctx:selected"] = "entity:ship",
        ["ctx:selected:ship:class"] = "colonizer",
        command = "ship.colonize",
    },
    function(_)
        return stack {
            action("Colonize", "ship.colonize"),
        }
    end
)

-- System / colony panels -----------------------------------------------------

fragment(
    "core.ui.system_panel.summary",
    { "window", "system", "summary", "selected_system" },
    { requires = { "system" }, optional = { "empire", "knowledge" } },
    "System Summary",
    function(_)
        return stack {
            row { action("Back to Galaxy", "selection.clear_system"), ui.text("<system name>") },
            ui.grid {
                columns = 2,
                children = {
                    ui.text("Star"), ui.text("<type>"),
                    ui.text("Owner"), ui.text("<empire>"),
                    ui.text("Position"), ui.text("<x,y,z>"),
                },
            },
            ui.section { title = "Planets", children = { note("host should query planet-list fragments") } },
            ui.section { title = "System Buildings", children = { note("needs build queue + action list") } },
        }
    end
)

fragment(
    "core.ui.system_panel.planet_list",
    { "system", "planet", "list", "selection" },
    { requires = { "system" }, optional = { "selected_planet" } },
    "Planet List",
    function(_)
        return stack {
            row { ui.text("Planet I"), ui.text("Temperate"), ui.text("Colony"), action("Open", "selection.planet") },
            row { ui.text("Planet II"), ui.text("Barren"), ui.text("Uncolonized"), action("Open", "selection.planet") },
            note("needs selectable/table primitive and sort headers"),
        }
    end
)

fragment(
    "core.ui.planet_window",
    { "window", "planet", "detail" },
    { requires = { "planet" }, optional = { "system", "colony", "empire" } },
    "Planet Detail",
    function(_)
        return stack {
            ui.grid {
                columns = 2,
                children = {
                    ui.text("Type"), ui.text("<planet type>"),
                    ui.text("Habitability"), ui.text("80%"),
                    ui.text("Minerals"), ui.text("Rich"),
                },
            },
            note("host should include colony fragments when colony context exists"),
        }
    end
)

fragment(
    "core.ui.colony.overview",
    { "colony", "detail", "tab", "overview", "buildings", "stockpile" },
    { requires = { "colony" }, optional = { "planet", "system", "empire" } },
    "Colony Overview",
    function(_)
        return stack {
            ui.grid {
                columns = 4,
                children = {
                    ui.text("Minerals"), ui.text("0"),
                    ui.text("Energy"), ui.text("0"),
                    ui.text("Food"), ui.text("0"),
                    ui.text("Authority"), ui.text("0"),
                },
            },
            ui.section {
                title = "Buildings",
                children = {
                    stack {
                        row { ui.text("Mine"), ui.progress(1.0), action("Demolish", "colony.demolish_building") },
                        row { ui.text("Power Plant"), ui.progress(0.4), action("Cancel", "colony.cancel_build_order") },
                    },
                },
            },
            ui.section {
                title = "Build",
                children = {
                    row { action("Mine", "colony.enqueue_building"), action("Farm", "colony.enqueue_building"), action("Lab", "colony.enqueue_building") },
                },
            },
        }
    end
)

fragment(
    "core.ui.colony.pop_management",
    { "colony", "detail", "tab", "population", "jobs" },
    { requires = { "colony" }, optional = { "planet", "empire" } },
    "Population Management",
    function(_)
        return stack {
            ui.grid {
                columns = 5,
                children = {
                    ui.text("Job"), ui.text("Assigned"), ui.text("Output"), ui.text("-"), ui.text("+"),
                    ui.text("Farmer"), ui.text("3"), ui.text("+9 Food"), action("-", "colony.job.dec"), action("+", "colony.job.inc"),
                    ui.text("Miner"), ui.text("2"), ui.text("+6 Minerals"), action("-", "colony.job.dec"), action("+", "colony.job.inc"),
                },
            },
            note("number_stepper primitive would simplify +/- action rows"),
        }
    end
)

-- Ship panels ----------------------------------------------------------------

fragment(
    "core.ui.ship.selection_multi",
    { "window", "ship", "selection", "multi", "fleet" },
    { requires = { "ships" }, optional = { "empire", "selected_system" } },
    "Selected Ships",
    function(_)
        return stack {
            row { action("Form Fleet", "fleet.form"), action("Merge Fleets", "fleet.merge"), action("Clear Selection", "selection.clear_ships") },
            ui.grid {
                columns = 3,
                children = {
                    ui.text("Ship"), ui.text("State"), ui.text("Fleet"),
                    ui.text("Scout-1"), ui.text("Idle"), ui.text("-"),
                    ui.text("Frigate-1"), ui.text("Patrol"), ui.text("1st Fleet"),
                },
            },
        }
    end
)

fragment(
    "core.ui.ship.detail",
    { "window", "ship", "detail", "commands", "cargo", "route" },
    { requires = { "ship" }, optional = { "empire", "fleet", "system" } },
    "Selected Ship",
    function(_)
        return stack {
            ui.grid {
                columns = 2,
                children = {
                    ui.text("State"), ui.text("<state>"),
                    ui.text("HP"), ui.text("100/100"),
                    ui.text("Cargo"), ui.text("0/10"),
                },
            },
            row { action("Cancel Current Action", "ship.cancel_current"), action("Clear All", "ship.clear_queue") },
            ui.section {
                title = "Cargo",
                children = {
                    row { action("Load M +100", "ship.load_minerals"), action("Load E +100", "ship.load_energy") },
                    row { action("Unload M", "ship.unload_minerals"), action("Unload E", "ship.unload_energy") },
                },
            },
            ui.section {
                title = "Route",
                children = {
                    row { action("Start Route", "ship.route.start"), action("Stop Route", "ship.route.stop") },
                },
            },
            note("needs select/dropdown primitive for ROE/courier mode"),
        }
    end
)

fragment(
    "core.ui.ship.refit",
    { "ship", "detail", "refit", "designs", "action_heavy" },
    { requires = { "ship" }, optional = { "empire", "ship_design_registry" } },
    "Ship Refit",
    function(_)
        return stack {
            ui.grid {
                columns = 3,
                children = {
                    ui.text("Design"), ui.text("Cost"), ui.text("Action"),
                    ui.text("Frigate Mk II"), ui.text("120 M"), action("Apply Refit", "ship.refit"),
                    ui.text("Fleet variant"), ui.text("600 M"), action("Apply to Fleet", "fleet.refit"),
                },
            },
        }
    end
)

-- Major windows --------------------------------------------------------------

bare_fragment(
    "core.ui.research",
    { "window", "research", "tabs", "tech_tree", "action_heavy" },
    { requires = { "empire" }, optional = { "research_queue", "tech_tree" } },
    function(view)
        local children = {
            row {
                ui.text("Research Pool " .. value_or(view.research_pool, "0 RP/hd")),
            },
        }

        if view.current then
            local current_rows = {
                kv("Project", value_or(view.current.name, "")),
                ui.progress(value_or(view.current.progress, 0)),
                ui.text(value_or(view.current.progress_label, "")),
                action("Cancel Research", "research.cancel"),
            }
            if view.current.blocked then
                table.insert(current_rows, ui.text("[Blocked]"))
            end
            table.insert(children, ui.section { title = "In Progress", children = { stack(current_rows) } })
        else
            table.insert(children, ui.text("No active research project."))
        end

        local tabs = {}
        local selected_branch = nil
        for _, branch in ipairs(value_or(view.branches, {})) do
            table.insert(tabs, {
                label = value_or(branch.name, "Branch"),
                command = value_or(branch.command, ""),
                selected = value_or(branch.selected, false),
            })
            if branch.selected then
                selected_branch = branch
            end
        end
        if #tabs == 0 then
            table.insert(children, ui.text("No tech branches defined."))
            return stack(children)
        end

        table.insert(children, ui.tabs { tabs = tabs })

        local tech_nodes = {}
        for _, tech in ipairs(value_or(value_or(selected_branch, {}).techs, {})) do
            local rows = {
                row {
                    ui.text(value_or(tech.status, "")),
                    ui.text(value_or(tech.name, "")),
                    ui.text(value_or(tech.cost, "")),
                },
            }
            if tech.dangerous then
                table.insert(rows, ui.text("[!] Dangerous"))
            end
            if value_or(tech.description, "") ~= "" then
                table.insert(rows, ui.text(tech.description))
            end
            for _, effect in ipairs(value_or(tech.effects, {})) do
                table.insert(rows, ui.text("Effect: " .. effect))
            end
            for _, unlock in ipairs(value_or(tech.unlocks, {})) do
                table.insert(rows, ui.text("Unlock: " .. unlock))
            end
            for _, requirement in ipairs(value_or(tech.missing_requirements, {})) do
                table.insert(rows, ui.text("Requires: " .. requirement))
            end
            if tech.command then
                table.insert(rows, action(value_or(tech.action_label, "Start Research"), tech.command, { disabled = value_or(tech.disabled, false) }))
            elseif tech.note then
                table.insert(rows, ui.text(tech.note))
            end
            table.insert(tech_nodes, ui.section { title = value_or(tech.name, "Technology"), children = { stack(rows) } })
        end
        table.insert(children, stack(tech_nodes))

        return stack(children)
    end
)

fragment(
    "core.ui.ship_designer",
    { "window", "ship_designer", "forms", "designs", "action_heavy" },
    { requires = { "empire" }, optional = { "hulls", "modules", "ship_designs" } },
    "Ship Designer",
    function(_)
        return ui.hstack {
            gap = "lg",
            children = {
                ui.section {
                    title = "Designs",
                    children = {
                        stack {
                            action("(new design)", "designer.new"),
                            action("Scout", "designer.select"),
                            action("Frigate", "designer.select"),
                        },
                    },
                },
                ui.section {
                    title = "Editor",
                    children = {
                        stack {
                            kv("Name", "<text input needed>"),
                            kv("Hull", "<select needed>"),
                            ui.grid {
                                columns = 2,
                                children = {
                                    ui.text("Weapon Slot"), action("Laser", "designer.slot.set"),
                                    ui.text("Utility Slot"), action("Armor", "designer.slot.set"),
                                },
                            },
                            row { action("New", "designer.new"), action("Save", "designer.save") },
                        },
                    },
                },
            },
        }
    end
)

fragment(
    "core.ui.diplomacy",
    { "window", "diplomacy", "relations", "options", "action_heavy" },
    { requires = { "empire" }, optional = { "target_faction", "relations" } },
    "Diplomacy",
    function(view)
        local children = {}

        local wars = value_or(view.active_wars, {})
        if #wars > 0 then
            local war_children = {}
            for _, war in ipairs(wars) do
                local rows = {
                    ui.text(value_or(war.title, "War")),
                    kv("Duration", value_or(war.duration, "0 hd")),
                    kv("Casus Belli", value_or(war.casus_belli, "")),
                }
                for _, demand in ipairs(value_or(war.demands, {})) do
                    table.insert(rows, ui.text("Demand: " .. demand))
                end
                local scenario_buttons = {}
                for _, scenario in ipairs(value_or(war.end_scenarios, {})) do
                    table.insert(scenario_buttons, action(value_or(scenario.label, "End War"), scenario.command))
                end
                if #scenario_buttons > 0 then
                    table.insert(rows, row(scenario_buttons))
                end
                table.insert(war_children, ui.section { title = value_or(war.opponent, "Opponent"), children = { stack(rows) } })
            end
            table.insert(children, ui.section { title = "Active Wars", children = { stack(war_children) } })
        end

        local faction_cards = {}
        for _, faction in ipairs(value_or(view.factions, {})) do
            local rows = {
                ui.text(value_or(faction.name, "Unknown")),
                kv("State", value_or(faction.state, "Neutral")),
                kv("Standing", value_or(faction.standing_label, "0")),
                ui.progress(value_or(faction.standing_progress, 0.5)),
            }
            local option_buttons = {}
            for _, option in ipairs(value_or(faction.options, {})) do
                table.insert(option_buttons, action(value_or(option.label, "Option"), option.command))
            end
            if #option_buttons > 0 then
                table.insert(rows, row(option_buttons))
            else
                table.insert(rows, ui.text("(No diplomatic options)"))
            end
            table.insert(faction_cards, ui.section { title = value_or(faction.name, "Faction"), children = { stack(rows) } })
        end

        if #faction_cards == 0 then
            table.insert(faction_cards, ui.text("No known factions."))
        end
        table.insert(children, ui.section { title = "Known Factions", children = { stack(faction_cards) } })

        return stack(children)
    end
)

fragment(
    "core.ui.lua_console",
    { "window", "debug", "lua_console", "developer" },
    { optional = { "log_buffer" } },
    "Lua Console",
    function(view)
        local lines = {}
        for _, entry in ipairs(value_or(view.entries, {})) do
            table.insert(lines, ui.text(value_or(entry.text, "")))
        end
        if #lines == 0 then
            table.insert(lines, ui.text("(no console output)"))
        end
        return stack {
            ui.section {
                title = "Log",
                children = {
                    stack(lines),
                },
            },
            note("input is hosted by the game until text_input is a DSL primitive"),
        }
    end
)

fragment(
    "core.ui.choice_dialog",
    { "modal", "choice", "blocking", "event" },
    { requires = { "choice" }, optional = { "empire" } },
    "Choice Dialog",
    function(view)
        local options = {}
        for _, option in ipairs(value_or(view.options, {})) do
            local rows = {
                ui.text(value_or(option.label, "Option")),
            }
            if value_or(option.description, "") ~= "" then
                table.insert(rows, ui.text(option.description))
            end
            if value_or(option.unmet_reason, "") ~= "" then
                table.insert(rows, ui.text(option.unmet_reason))
            end
            table.insert(rows, action("Choose", option.command, { disabled = value_or(option.disabled, false) }))
            table.insert(options, ui.section { title = value_or(option.label, "Option"), children = { stack(rows) } })
        end
        return stack {
            ui.text(value_or(view.description, "")),
            ui.section {
                title = value_or(view.title, "Choice"),
                children = {
                    stack(options),
                },
            },
        }
    end
)

-- Empire Situation Center ----------------------------------------------------

bare_fragment(
    "core.ui.esc.notifications",
    { "window", "esc", "tab", "notifications", "ack" },
    { requires = { "empire" }, optional = { "notification_queue" } },
    { esc_tab = "notifications" },
    function(view)
        local function filter_action(label, key)
            local text = label
            if view.severity_filter == key then
                text = "[" .. label .. "]"
            end
            return action(text, "esc.notifications.filter." .. key)
        end

        local function acked_filter_label()
            if view.hide_acked then
                return "[Hide acked]"
            end
            return "Hide acked"
        end

        local function notification_node(notif, depth)
            local children = {}
            local prefix = string.rep("  ", depth)
            local message = value_or(notif.message, "")
            if value_or(notif.acked, false) then
                message = message .. " (acked)"
            end

            local row_children = {
                ui.text(prefix .. value_or(notif.severity_label, "INFO")),
                ui.text(message),
                ui.text("t=" .. tostring(value_or(notif.timestamp, 0))),
            }
            if not value_or(notif.acked, false) then
                table.insert(row_children, action("ack", "esc.notifications.ack:" .. tostring(notif.id)))
            end
            table.insert(children, row(row_children))

            local nested = value_or(notif.children, {})
            for _, child in ipairs(nested) do
                table.insert(children, notification_node(child, depth + 1))
            end

            return stack(children)
        end

        local notification_children = {}
        local notifications = value_or(view.notifications, {})
        if #notifications == 0 then
            table.insert(notification_children, ui.text("(no notifications)"))
        else
            for _, notif in ipairs(notifications) do
                table.insert(notification_children, notification_node(notif, 0))
            end
        end

        return stack {
            row {
                filter_action("All", "all"),
                filter_action("Info+", "info"),
                filter_action("Warn+", "warn"),
                filter_action("Critical", "critical"),
                action(acked_filter_label(), "esc.notifications.hide_acked.toggle"),
            },
            row {
                action("Ack all", "esc.notifications.ack_all"),
                ui.text(tostring(value_or(view.unacked_count, 0)) .. " unacked"),
            },
            ui.section {
                title = "Notifications",
                children = { stack(notification_children) },
            },
        }
    end
)

bare_fragment(
    "core.ui.esc.construction",
    { "window", "esc", "tab", "construction", "ongoing" },
    { requires = { "empire" }, optional = { "systems", "colonies" } },
    { esc_tab = "construction_overview" },
    function(view)
        return event_tree(view, "(nothing under construction)")
    end
)

bare_fragment(
    "core.ui.esc.ship_ops",
    { "window", "esc", "tab", "ship_ops", "ongoing" },
    { requires = { "empire" }, optional = { "ships", "fleets" } },
    { esc_tab = "ship_operations" },
    function(view)
        return event_tree(view, "(no ship operations)")
    end
)

bare_fragment(
    "core.ui.esc.diplomacy",
    { "window", "esc", "tab", "diplomacy", "ongoing" },
    { requires = { "empire" }, optional = { "relations" } },
    { esc_tab = "diplomatic_standing" },
    function(view)
        return event_tree(view, "(no known factions)")
    end
)

bare_fragment(
    "core.ui.esc.resource_trends",
    { "window", "esc", "tab", "resources", "charts" },
    { requires = { "empire" }, optional = { "resource_history" } },
    { esc_tab = "resource_trends" },
    function(view)
        return event_tree(view, "(no resource samples yet)")
    end
)

-- AI debug -------------------------------------------------------------------

fragment(
    "core.ui.ai_debug.inspector",
    { "window", "debug", "ai", "tab", "inspector" },
    { optional = { "ai_debug" } },
    "AI Debug Inspector",
    function(_)
        return ui.hstack {
            gap = "lg",
            children = {
                ui.section { title = "Entities", children = { stack { action("Faction A", "ai_debug.select"), action("Fleet 1", "ai_debug.select") } } },
                ui.section { title = "Details", children = { stack { kv("Goal", "Expand"), kv("State", "Planning") } } },
            },
        }
    end
)

fragment(
    "core.ui.ai_debug.plots",
    { "window", "debug", "ai", "tab", "plots", "charts" },
    { optional = { "ai_debug" } },
    "AI Debug Plots",
    function(_)
        return ui.hstack {
            gap = "lg",
            children = {
                ui.section { title = "Window", children = { stack { action("100", "ai_debug.plots.window"), action("500", "ai_debug.plots.window") } } },
                ui.section { title = "Plot", children = { stack { row { ui.text("Score"), ui.progress(0.6) }, note("chart primitive needed") } } },
            },
        }
    end
)

fragment(
    "core.ui.ai_debug.stream",
    { "window", "debug", "ai", "tab", "stream" },
    { optional = { "ai_debug" } },
    "AI Debug Stream",
    function(_)
        return stack {
            row { action("Pause", "ai_debug.stream.pause"), action("Clear", "ai_debug.stream.clear"), action("All", "ai_debug.stream.filter") },
            ui.section {
                title = "Events",
                children = {
                    stack {
                        ui.text("[ai] decision tick"),
                        ui.text("[ai] command emitted"),
                    },
                },
            },
        }
    end
)

fragment(
    "core.ui.ai_debug.governor",
    { "window", "debug", "ai", "tab", "governor" },
    { optional = { "ai_debug" } },
    "AI Debug Governor",
    function(_)
        return stack {
            ui.section { title = "Economy", children = { ui.grid { columns = 2, children = { ui.text("Minerals"), ui.text("120"), ui.text("Energy"), ui.text("80") } } } },
            ui.section { title = "Military", children = { ui.grid { columns = 2, children = { ui.text("Fleets"), ui.text("2"), ui.text("Power"), ui.text("42") } } } },
            note("collapsible group primitive would fit this tab better"),
        }
    end
)

fragment(
    "core.ui.ai_debug.replay",
    { "window", "debug", "ai", "tab", "replay", "file_io" },
    { optional = { "ai_debug" } },
    "AI Debug Replay",
    function(_)
        return stack {
            row { ui.text("<file path input needed>"), action("Load", "ai_debug.replay.load"), action("Unload", "ai_debug.replay.unload") },
            row { action("|<", "ai_debug.replay.start"), action("<", "ai_debug.replay.prev"), action(">", "ai_debug.replay.next"), action(">|", "ai_debug.replay.end") },
            ui.section { title = "Frame", children = { stack { kv("Tick", "0"), kv("Event", "<none>") } } },
        }
    end
)
