**Next release** fixes Auto-orient for filament printing.

## Fixed

- **Auto-orient** could turn a model upside down, leaving it needing far more support than before. It now estimates how much support each way of standing the model would need, then refines the best few turns in small steps. On the starter bear it settles upright, with slightly less support than the original pose, whatever way it started.
