# Project guidance

## Comment style

Code comments must describe **what the code does and why, in terms of the code itself**. They are read by people staring at the function, not at the PR that introduced it.

**Do:**

- State the invariant the code preserves, the contract with its caller, or the ordering requirement it relies on.
- Name the specific functions, fields, or modules the code interacts with (`monitor::service::handle_error`, `streamer_manager.reload_from_repo`, `live_sessions.end_time`, ...). Concrete references age well; readers can `grep` them.
- Explain why an error is logged-and-continued vs. propagated, why a check is gated on a flag, or why two operations must run in a particular order.

**Do not:**

- Reference internal design labels invented in a PR body or chat thread ("Strategy B", "the kinetic gap fix", etc.). They are unfamiliar to anyone who didn't read that PR.
- Cite PR numbers, issue numbers, or commit hashes inline. If a comment depends on that history, the comment is documenting the wrong thing — push the explanation into the *what* of the surrounding code or drop it.
- Describe pre-fix behaviour, regression history, or what the code "used to do". The diff already shows that; the comment is for the current code.
- Repeat the user-facing bug-report wording (e.g. "showing offline on web while recording is in flight"). That belongs in the release notes / PR body, not in code.
- Narrate. One precise sentence beats three about how we got here.

When a comment naturally wants to reach for forbidden context, the underlying explanation is almost always recoverable in code-local terms: name the function whose behaviour creates the constraint, name the field that must hold a specific value before another operation runs, name the call ordering between modules. Those references stay correct as the codebase evolves; design-history references rot the day the PR closes.
