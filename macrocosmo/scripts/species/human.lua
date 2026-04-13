-- #241: Species-level modifiers replace `job_bonuses`. Use job-scoped targets
-- (`job:<id>::colony.<X>`) so the bonus applies only when the pop is assigned
-- to that job, and colony-scoped targets (`colony.<X>`) for species-wide
-- bonuses that apply regardless of assignment.
local human = define_species {
    id = "human",
    name = "Human",
    growth_rate = 0.01,
    modifiers = {
        -- +10% research output for researchers (matches legacy job_bonuses
        -- value of 0.1 on the researcher role).
        { target = "job:researcher::colony.research_per_hexadies", multiplier = 0.1 },
    },
}

return {
    human = human,
}
