local ui = require("macrocosmo.ui")

local function fragment(id, labels, title, body)
    return define_ui_fragment {
        id = id,
        labels = labels,
        render = function(view)
            return ui.section {
                title = title,
                children = { body(view) },
            }
        end,
    }
end

local function row(children)
    return ui.hstack { children = children }
end

local function stack(children)
    return ui.vstack { children = children }
end

local function kv(key, value)
    return row { ui.text(key), ui.text(value) }
end

local function button(label, command)
    return ui.button { label = label, command = command }
end

local function disabled_button(label, command, disabled_when)
    return ui.button {
        label = label,
        command = command,
        disabled = true,
        disabled_when = disabled_when,
    }
end

local function action(label, command)
    return ui.action { label = label, command = command }
end

fragment(
    "catalogue.atom.text",
    { "catalogue", "atom", "text", "static_content" },
    "Text Atoms",
    function(_)
        return stack {
            ui.text("Plain body text"),
            ui.text("Short status: Idle"),
            ui.text("Long copy should remain renderable even before wrapping policy is finalized."),
            kv("Key", "Value"),
            kv("Signed value", "+12"),
            kv("Placeholder", "<host supplied value>"),
        }
    end
)

fragment(
    "catalogue.atom.layout",
    { "catalogue", "atom", "layout", "composition" },
    "Layout Atoms",
    function(_)
        return stack {
            ui.section {
                title = "Vertical stack",
                children = {
                    stack {
                        ui.text("First"),
                        ui.text("Second"),
                        ui.text("Third"),
                    },
                },
            },
            ui.section {
                title = "Horizontal row",
                children = {
                    row {
                        ui.text("Left"),
                        ui.text("Center"),
                        ui.text("Right"),
                    },
                },
            },
            ui.section {
                title = "Row alias",
                children = {
                    ui.row {
                        children = {
                            ui.text("Label"),
                            ui.text("Value"),
                            button("Action", "catalogue.action"),
                        },
                    },
                },
            },
        }
    end
)

fragment(
    "catalogue.atom.grid",
    { "catalogue", "atom", "grid", "comparison", "table" },
    "Grid Atoms",
    function(_)
        return stack {
            ui.grid {
                columns = 3,
                children = {
                    ui.text("Metric"), ui.text("Current"), ui.text("Delta"),
                    ui.text("Minerals"), ui.text("120"), ui.text("+12"),
                    ui.text("Energy"), ui.text("80"), ui.text("-3"),
                    ui.text("Food"), ui.text("64"), ui.text("+8"),
                },
            },
            ui.grid {
                columns = 2,
                children = {
                    ui.text("Owner"), ui.text("Terran Union"),
                    ui.text("State"), ui.text("Surveying"),
                    ui.text("ETA"), ui.text("42"),
                },
            },
        }
    end
)

fragment(
    "catalogue.atom.progress",
    { "catalogue", "atom", "progress", "meter", "status" },
    "Progress Atoms",
    function(_)
        return stack {
            row { ui.text("Empty"), ui.progress(0.0) },
            row { ui.text("Partial"), ui.progress(0.35) },
            row { ui.text("Complete"), ui.progress(1.0) },
            row { ui.text("Clamped low"), ui.progress(-0.25) },
            row { ui.text("Clamped high"), ui.progress(1.25) },
        }
    end
)

fragment(
    "catalogue.atom.tooltip",
    { "catalogue", "atom", "tooltip", "explanation", "hover" },
    "Tooltip Atom",
    function(_)
        return stack {
            ui.tooltip {
                content = ui.text("Hoverable status"),
                tooltip = {
                    ui.text("This is a generic tooltip wrapper."),
                    ui.text("It can explain any child node, not only values."),
                },
            },
            ui.tooltip {
                content = button("Why disabled?"),
                tooltip = ui.grid {
                    columns = 2,
                    children = {
                        ui.text("Condition"), ui.text("State"),
                        ui.text("Has colony"), ui.text("true"),
                        ui.text("Enough minerals"), ui.text("false"),
                    },
                },
            },
        }
    end
)

fragment(
    "catalogue.atom.modified_value",
    { "catalogue", "atom", "modified_value", "tooltip", "modifier_breakdown" },
    "Modified Value Atom",
    function(_)
        return stack {
            ui.modified_value {
                label = "Mining Output",
                base = "10",
                final = "19",
                modifiers = {
                    {
                        label = "Automated Mining",
                        parts = { "+2 (base add)", "x1.5 (mult)" },
                    },
                    {
                        label = "Low Stability",
                        parts = { "-1 (add)" },
                        remaining_duration = 12,
                    },
                },
            },
            ui.modified_value {
                label = "Ship Range",
                base = "6",
                final = "6",
                modifiers = {},
            },
        }
    end
)

fragment(
    "catalogue.atom.command",
    { "catalogue", "atom", "button", "action", "command" },
    "Command Atoms",
    function(_)
        return stack {
            ui.section {
                title = "Optional command button",
                children = {
                    row {
                        button("No command"),
                        button("Open", "ui.open"),
                        button("Cancel", "ui.cancel"),
                    },
                },
            },
            ui.section {
                title = "Required command action",
                children = {
                    row {
                        action("Move", "ship.move"),
                        action("Survey", "ship.survey"),
                        action("Ack all", "notifications.ack_all"),
                    },
                },
            },
            ui.section {
                title = "Disabled command",
                children = {
                    row {
                        disabled_button(
                            "Build Mine",
                            "colony.build_mine",
                            {
                                label = "Can build mine",
                                satisfied = false,
                                op = "all",
                                children = {
                                    { label = "Has colony", satisfied = true },
                                    { label = "Enough minerals", satisfied = false },
                                    { label = "Free building slot", satisfied = true },
                                },
                            }
                        ),
                    },
                    row {
                        disabled_button(
                            "Start Megaproject",
                            "project.start_megastructure",
                            {
                                label = "Can start orbital habitat project",
                                satisfied = false,
                                op = "any",
                                children = {
                                    {
                                        label = "Has a colony with population over 500",
                                        satisfied = false,
                                    },
                                    {
                                        label = "Can construct advanced habitat",
                                        satisfied = false,
                                        op = "all",
                                        children = {
                                            {
                                                label = "Orbital Habitats researched",
                                                satisfied = true,
                                            },
                                            {
                                                label = "Climate Engineering researched",
                                                satisfied = false,
                                            },
                                            {
                                                label = "No active blockade",
                                                satisfied = false,
                                                op = "not",
                                                children = {
                                                    {
                                                        label = "Hostile fleet in orbit",
                                                        satisfied = true,
                                                    },
                                                },
                                            },
                                        },
                                    },
                                },
                            }
                        ),
                    },
                },
            },
        }
    end
)

fragment(
    "catalogue.atom.grouping",
    { "catalogue", "atom", "section", "grouping", "hierarchy" },
    "Grouping Atoms",
    function(_)
        return stack {
            ui.section {
                title = "Primary group",
                children = {
                    stack {
                        kv("Name", "Sol"),
                        kv("Owner", "Terran Union"),
                    },
                },
            },
            ui.section {
                title = "Nested groups",
                children = {
                    stack {
                        ui.section {
                            title = "Inner A",
                            children = { ui.text("Nested content A") },
                        },
                        ui.section {
                            title = "Inner B",
                            children = { ui.text("Nested content B") },
                        },
                    },
                },
            },
        }
    end
)

fragment(
    "catalogue.pattern.summary_panel",
    { "catalogue", "pattern", "summary", "read_only" },
    "Read-only Summary Pattern",
    function(_)
        return stack {
            ui.grid {
                columns = 2,
                children = {
                    ui.text("Name"), ui.text("Scout-1"),
                    ui.text("State"), ui.text("Surveying"),
                    ui.text("Location"), ui.text("Sol"),
                    ui.text("Cargo"), ui.text("20 / 100"),
                },
            },
            row { ui.text("Hull"), ui.progress(0.82) },
        }
    end
)

fragment(
    "catalogue.pattern.action_panel",
    { "catalogue", "pattern", "actions", "command_surface" },
    "Action Panel Pattern",
    function(_)
        return stack {
            ui.section {
                title = "Primary actions",
                children = {
                    row {
                        action("Move", "ship.move"),
                        action("Survey", "ship.survey"),
                        action("Colonize", "ship.colonize"),
                    },
                },
            },
            ui.section {
                title = "Secondary actions",
                children = {
                    row {
                        button("Details", "ui.open.details"),
                        button("Cancel", "ui.cancel"),
                    },
                },
            },
        }
    end
)
