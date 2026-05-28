-- Lua UI DSL shadow definitions for existing Rust UI.
--
-- One fragment is currently wired into the ESC "Lua UI" preview tab. The rest
-- pressure-test the DSL authoring shape using the current primitive set:
-- section / vstack / hstack / grid / row / text / progress / button / action.

local ui = require("macrocosmo.ui")

local function stack(children)
    return ui.vstack { gap = "sm", children = children }
end

local function row(children)
    return ui.hstack { gap = "sm", children = children }
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

local function action(label, command)
    return ui.button { label = label, command = command }
end

local function note(text)
    return ui.text("TODO: " .. text)
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

-- Frame chrome ---------------------------------------------------------------

fragment(
    "core.ui.top_bar",
    { "chrome", "top_bar", "global", "time", "resources", "action_entrypoints" },
    { requires = { "empire" }, optional = { "clock", "observer_view" } },
    "Top Bar",
    function(_)
        return row {
            ui.text("HD 0000"),
            action("Pause", "time.pause"),
            action("Play", "time.play"),
            action("Fast", "time.fast"),
            ui.grid {
                columns = 8,
                children = {
                    ui.text("Minerals"), ui.text("+0"),
                    ui.text("Energy"), ui.text("+0"),
                    ui.text("Food"), ui.text("+0"),
                    ui.text("Authority"), ui.text("+0"),
                },
            },
            action("Research", "ui.open.research"),
            action("Diplomacy", "ui.open.diplomacy"),
            action("Designer", "ui.open.ship_designer"),
            action("ESC", "ui.toggle.situation_center"),
        }
    end
)

fragment(
    "core.ui.bottom_bar",
    { "chrome", "bottom_bar", "event_log" },
    { optional = { "event_log" } },
    "Bottom Bar",
    function(_)
        return row {
            ui.text("[hd 0000] Event log entry"),
            ui.text("[hd 0001] Another event"),
            note("needs horizontal scroll or clipping policy for low-height chrome"),
        }
    end
)

fragment(
    "core.ui.notification_pills",
    { "overlay", "notifications", "transient" },
    { requires = { "empire" }, optional = { "notification_queue" } },
    "Notification Pills",
    function(_)
        return ui.vstack {
            gap = "xs",
            children = {
                row { ui.text("High"), ui.text("Colony established"), action("Jump", "ui.jump.notification") },
                row { ui.text("Info"), ui.text("Survey complete"), action("Ack", "notification.ack") },
            },
        }
    end
)

-- Navigation / selection -----------------------------------------------------

fragment(
    "core.ui.outline",
    { "side_panel", "outline", "navigation", "systems", "ships" },
    { requires = { "empire" }, optional = { "selected_system", "selected_ship", "knowledge" } },
    "Outline",
    function(_)
        return stack {
            ui.section {
                title = "Systems",
                children = {
                    stack {
                        row { ui.text("> Sol"), action("Select", "selection.system") },
                        row { ui.text("  Alpha Centauri"), action("Select", "selection.system") },
                    },
                },
            },
            ui.section {
                title = "Ships",
                children = {
                    stack {
                        row { ui.text("Scout-1"), ui.text("Surveying"), action("Select", "selection.ship") },
                        row { ui.text("Constructor-1"), ui.text("Idle"), action("Select", "selection.ship") },
                    },
                },
            },
            note("tree/collapsing primitive would replace text arrows"),
        }
    end
)

fragment(
    "core.ui.context_menu.ship_commands",
    { "modal", "context_menu", "ship", "commands", "action_heavy" },
    { requires = { "ship", "target_system" }, optional = { "empire", "target_planet" } },
    "Ship Commands",
    function(_)
        return stack {
            kv("Ship", "<selected ship>"),
            kv("Target", "<target system>"),
            action("Move", "ship.move"),
            action("Survey", "ship.survey"),
            action("Colonize", "ship.colonize"),
            action("Cancel", "ui.close.context_menu"),
            note("needs disabled reasons and command previews"),
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

fragment(
    "core.ui.research",
    { "window", "research", "tabs", "tech_tree", "action_heavy" },
    { requires = { "empire" }, optional = { "research_queue", "tech_tree" } },
    "Research",
    function(_)
        return stack {
            row { action("Physics", "research.branch.physics"), action("Industrial", "research.branch.industrial"), action("Social", "research.branch.social"), action("Military", "research.branch.military") },
            ui.section {
                title = "Current",
                children = {
                    row { ui.text("Automated Mining"), ui.progress(0.6), action("Cancel Research", "research.cancel") },
                },
            },
            ui.section {
                title = "Available",
                children = {
                    stack {
                        row { ui.text("FTL Theory"), ui.text("100"), action("Research", "research.start") },
                        row { ui.text("Habitat Engineering"), ui.text("150"), action("Research", "research.start") },
                    },
                },
            },
            note("needs tab strip/selectable primitive and prerequisite tooltip support"),
        }
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
    function(_)
        return ui.hstack {
            gap = "lg",
            children = {
                ui.section {
                    title = "Factions",
                    children = {
                        stack {
                            action("Terran Federation", "diplomacy.select_faction"),
                            action("Vesk Combine", "diplomacy.select_faction"),
                        },
                    },
                },
                ui.section {
                    title = "Relation",
                    children = {
                        stack {
                            kv("State", "Peace"),
                            kv("Standing", "+10"),
                            row { action("Declare War", "diplomacy.declare_war"), action("Offer Peace", "diplomacy.offer_peace") },
                            row { action("Open Borders", "diplomacy.option"), action("Trade", "diplomacy.option") },
                        },
                    },
                },
            },
        }
    end
)

fragment(
    "core.ui.lua_console",
    { "window", "debug", "lua_console", "developer" },
    { optional = { "log_buffer" } },
    "Lua Console",
    function(_)
        return stack {
            ui.section {
                title = "Log",
                children = {
                    stack {
                        ui.text("> print('hello')"),
                        ui.text("hello"),
                    },
                },
            },
            row { ui.text("<text input needed>"), action("Run", "lua_console.run") },
            note("needs text_input and command history state"),
        }
    end
)

fragment(
    "core.ui.choice_dialog",
    { "modal", "choice", "blocking", "event" },
    { requires = { "choice" }, optional = { "empire" } },
    "Choice Dialog",
    function(_)
        return stack {
            ui.text("<choice title/body>"),
            ui.section {
                title = "Options",
                children = {
                    stack {
                        row { ui.text("Option A"), ui.text("Effect preview"), action("Choose", "choice.select") },
                        row { ui.text("Option B"), ui.text("Hidden effect"), action("Choose", "choice.select") },
                    },
                },
            },
        }
    end
)

-- Empire Situation Center ----------------------------------------------------

fragment(
    "core.ui.esc.notifications",
    { "window", "esc", "tab", "notifications", "ack" },
    { requires = { "empire" }, optional = { "notification_queue" } },
    "ESC Notifications",
    function(_)
        return stack {
            row { action("All", "esc.notifications.filter_all"), action("Info+", "esc.notifications.filter_info"), action("Warn+", "esc.notifications.filter_warn"), action("Ack all", "esc.notifications.ack_all") },
            ui.section {
                title = "Notifications",
                children = {
                    stack {
                        row { ui.text("Critical"), ui.text("Hostile detected"), action("ack", "notification.ack") },
                        row { ui.text("Info"), ui.text("Survey complete"), action("ack", "notification.ack") },
                    },
                },
            },
            note("needs collapsible tree primitive"),
        }
    end
)

fragment(
    "core.ui.esc.construction",
    { "window", "esc", "tab", "construction", "ongoing" },
    { requires = { "empire" }, optional = { "systems", "colonies" } },
    "ESC Construction",
    function(_)
        return stack {
            row { ui.text("Sol Shipyard"), ui.progress(0.35), ui.text("[BOTTLENECK]"), action("Jump", "ui.jump.system") },
            row { ui.text("Mars Mine"), ui.progress(0.8), action("Jump", "ui.jump.colony") },
        }
    end
)

fragment(
    "core.ui.esc.ship_ops",
    { "window", "esc", "tab", "ship_ops", "ongoing" },
    { requires = { "empire" }, optional = { "ships", "fleets" } },
    "ESC Ship Operations",
    function(_)
        return stack {
            ui.section { title = "Survey", children = { row { ui.text("Scout-1 -> Alpha"), ui.progress(0.5), action("Jump", "ui.jump.ship") } } },
            ui.section { title = "Transit", children = { row { ui.text("Constructor-1"), ui.text("ETA 42"), action("Jump", "ui.jump.ship") } } },
        }
    end
)

fragment(
    "core.ui.esc.diplomacy",
    { "window", "esc", "tab", "diplomacy", "ongoing" },
    { requires = { "empire" }, optional = { "relations" } },
    "ESC Diplomacy",
    function(_)
        return ui.grid {
            columns = 4,
            children = {
                ui.text("Faction"), ui.text("State"), ui.text("Standing"), ui.text("Action"),
                ui.text("Vesk"), ui.text("Peace"), ui.text("+10"), action("Open", "ui.open.diplomacy"),
                ui.text("Krell"), ui.text("War"), ui.text("-80"), action("Open", "ui.open.diplomacy"),
            },
        }
    end
)

fragment(
    "core.ui.esc.resource_trends",
    { "window", "esc", "tab", "resources", "charts" },
    { requires = { "empire" }, optional = { "resource_history" } },
    "ESC Resource Trends",
    function(_)
        return stack {
            row { ui.text("Minerals"), ui.progress(0.7), ui.text("+12") },
            row { ui.text("Energy"), ui.progress(0.4), ui.text("-3") },
            row { ui.text("Food"), ui.progress(0.9), ui.text("+8") },
            note("sparkline/chart primitive needed for actual trends"),
        }
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
