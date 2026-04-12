-- Lifecycle hooks: runs on game start/load/scripts loaded

on_game_start(function()
    -- Start periodic events when a new game begins
    fire_event("monthly_report")
end)
