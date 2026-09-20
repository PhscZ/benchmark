# AI Benchmark

An end-to-end coding benchmark comparing state-of-the-art AI models on software development tasks relevant to my use case.

Models are evaluated on whether their software runs and correctly completes the required tasks.

## Scoring

Each submission receives two separate scores:

- **Subjective (0–100):** My assessment of the implementation’s quality and how well it meets my needs.
- **Objective (0–100):** Calculated from the attempts required to run the software and its performance on five test tasks.

### Execution attempts

Each submission gets a maximum of **four attempts** to run.

| First successful run | Starting objective score |
|---|---:|
| Attempt 1 | 100 |
| Attempt 2 | 75 |
| Attempt 3 | 50 |
| Attempt 4 | 25 |
| Never runs | 0 |

Between attempts, I provide only the errors produced by the console.

### Functional tests

Once the software runs, it is evaluated against **five predefined test tasks**.

| Task result | Penalty |
|---|---|
| Passes fully | No deduction |
| Works partially, has missing functionality, or contains a functional bug | Deduct 10% of the remaining score |
| Fails | Deduct 20% of the remaining score |

**Penalties are multiplicative, not percentage-point deductions.** Passing tasks do not restore lost points.

```text
Objective score = starting score × 0.8^failed_tasks × 0.9^partial_tasks
```

### Example

A submission runs on its second attempt, fails one task, partially passes two, and fully passes two:

| Step | Calculation | Remaining score |
|---|---|---:|
| Initial score | — | 100 |
| Runs on attempt 2 | 100 × 0.75 | 75 |
| Task 1: fail | 75 × 0.80 | 60 |
| Task 2: partial | 60 × 0.90 | 54 |
| Task 3: partial | 54 × 0.90 | 48.6 |
| Task 4: pass | No deduction | 48.6 |
| Task 5: pass | No deduction | **48.6** |

> Under this scoring system, software that runs but fails all five tasks still receives a nonzero score. The objective score rewards both successful execution and functionality.

## Evaluation consistency

- Each model receives the same task specification and test requirements.
- Submissions are evaluated in the same environment for each challenge (Windows, Zed, omp)
- Pass, partial-pass, and failure criteria are defined before evaluation.
- Subjective and objective scores are reported separately, alongside the attempt count and test results.

Results reflect performance on **my selected use cases**, not a universal ranking of coding ability.
