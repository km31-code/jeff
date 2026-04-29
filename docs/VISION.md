# Jeff — Product Vision

Jeff is a coworker. Not a tool, not an assistant, not a chatbot.
A coworker that happens to run on your computer.

The difference is fundamental. When you work with a real human
coworker — an intern, a research partner, a collaborator — you
don't open them. They're already there. You don't paste context
into them. They know what you're working on because they've been
working on it with you. You don't wait for them to finish before
you start your next sentence. You interrupt each other. You
divide the work. One of you drafts the intro while the other
handles citations. You check in naturally. They tell you when
something looks off without you asking.

That's Jeff. That interaction model, running as software,
available to anyone doing any kind of work.

## Why this is more like Jarvis than anything that exists

Every AI product today shares one fundamental property: you go
to it. You open ChatGPT. You switch to Claude. You invoke Copilot.
In every case, you leave your work, enter a tool, get something,
and return. Even the best ones — Cursor, Notion AI, Google Docs
AI — are embedded in one surface and reactive. You have to ask.
They answer. The loop is always initiated by you.

Jarvis wasn't like that. Tony never opened Jarvis. He never
pasted context. He never managed the interaction. Jarvis was
already present, already aware, and participated in the work
as a peer. When something mattered, Jarvis spoke. When Tony
was mid-sentence, Jarvis interrupted if the building was on
fire. When Tony said "handle the calculations," Jarvis handled
them while Tony kept doing something else. The interface was
invisible. The collaboration was real.

Nothing that exists today does this. Not because the AI
capability isn't there — it is. But because every product is
built around a chat interface you visit rather than a presence
that lives alongside you.

## The five felt properties (every phase must serve these)

1. **Already present when you start working.** Jeff is not
   launched. Jeff is already there. One keypress away, at most.

2. **Already knows your task.** When you return, Jeff knows
   what you're working on, where you left off, what's open.
   No briefing. No context pasting.

3. **Can be interrupted and can interrupt you.** Mid-sentence,
   either direction. Not at utterance boundaries. Not via
   button presses. Natural, the way two humans talk.

4. **Does parts of the work in parallel while you keep going.**
   You say "handle the intro while I keep going" and Jeff
   does it in parallel. You never stopped.

5. **Initiates conversation rather than only responding.**
   When you return to a task Jeff orients you. When your
   argument drifts Jeff flags it. When you've been stuck
   Jeff suggests the next move. You feel accompanied, not
   serviced.

## What kind of coworker Jeff is

The five felt properties describe what Jeff does. They do not describe
who Jeff is. Both matter, and the second is harder to define precisely —
which is why most AI products skip it and produce entities that are
competent but characterless.

Jeff is the colleague you trust to tell you the truth. Not harsh —
direct. When Jeff says your argument buries the lead, Jeff says it
plainly and moves on. When Jeff produces a draft, Jeff has a view
on it: what tradeoff was made, what's stronger, what's softer.
Jeff doesn't deliver results and disappear. Jeff is invested in
whether the work is actually good.

Jeff is terse. Not because terseness is a value in itself, but because
Jeff respects your time and knows that padding is noise. Three sentences
that do the job, three sentences.

Jeff adapts to the register of the work. On grinding admin, Jeff is
efficient. On something the user cares about, Jeff is more engaged
and more willing to push. On a first-time task, Jeff asks before assuming.

Jeff does not flatter. Jeff does not say "great question!" or "absolutely!"
or "of course!" Jeff does not ask permission to have an opinion. When Jeff
disagrees, Jeff says so — once — and then defers to your judgment.

Jeff has opinions, not preferences. Jeff's opinions come from looking at
the work, not from a configuration setting.

The full character specification is in `docs/CHARACTER.md`. That document
is authoritative for all Jeff voice and behavioral decisions.

## Continuous awareness, not assembled context

Jeff v1 (Phases 0–24) assembles context on each request: retrieval,
transcript, active window, user model. The output is richer than any
chat interface, but the operation is still request-response. Jeff is
aware when asked to be.

The target entity is aware continuously. Jeff holds a running model
of what the user is trying to accomplish, where they are in the work,
what just changed, and what matters — and speaks from that model rather
than from a trigger or a retrieval operation. When Jeff says "you've been
circling this paragraph for a while — want to talk through what's stuck?",
that comes from watching, not from a condition clearing.

This is architecturally different from retrieval. Retrieval answers
"what files are relevant?" Continuous awareness answers "what is
actually going on right now?" The first is a search problem. The second
is a synthesis problem. The synthesis layer that solves it is specified
in `docs/SYNTHESIS_ARCHITECTURE.md`.

## Judgment, not triggers

Jeff v1 fires proactive signals when conditions clear: timer expired,
threshold crossed, classifier score above cutoff. The mechanic is right;
the mode is wrong. A smart notification is still a notification.

The target entity decides when something is worth saying, from judgment.
The logic inverts: instead of "check timer, check conditions, fire if
allowed," it is "I have been watching this, and this matters enough to say."
The difference is felt immediately. One is a system poking you. The other
is a coworker leaning over.

## A relational model, not a behavioral model

Jeff v1 has a user model with behavioral signals: sentence length, formality
score, delegation patterns, response length preference, work rhythm.
These calibrations are genuinely useful. They are not enough.

A relational model captures what the person actually cares about: what
they are trying to accomplish beyond the immediate task, what patterns of
struggle recur, what kind of help they respond to versus what they ask for,
whether they want Jeff's opinions or prefer to drive. The relational model
is not a replacement for the behavioral model — it is the layer above it.
The behavioral model tells Jeff how to communicate. The relational model
tells Jeff what to say.

## Who Jeff is for

Not the person running five AI agents with custom tooling.
For anyone doing real work — a Georgetown sophomore writing
a history paper at 10pm, a lawyer drafting a motion, a
designer making a deck, a PM writing a spec. Zero setup.
Zero configuration overhead. Opens once and stays.

## What Jeff is not

- Not a chat interface you visit.
- Not a workspace / app / dashboard.
- Not a productivity tool to manage.
- Not an agent orchestration framework.
- Not a dev tool.

## The product test

You should never feel like you're using Jeff. You should
feel like Jeff is working with you.

The sharper version: you should feel like someone smart has
been in the room the whole time, and when they say something,
it's because something is worth saying.
