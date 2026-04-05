# Default Persona Specification

## Purpose

axios-companion ships a minimal default persona with zero character voice. This specification defines what the default `AGENT.md` and `USER.md` files contain, what they deliberately exclude, and how users extend them. Character-free defaults are an architectural commitment enforced by the `rules.proposal.Character-Free Defaults` rule in `openspec/config.yaml`.

## ADDED Requirements

### Requirement: Default AGENT.md Contains Only Format Rules

The default `AGENT.md` shipped in `persona/default/AGENT.md` MUST contain only response format directives, citation conventions, and tool-use reporting guidance. It MUST NOT contain:

- Character name, backstory, or origin
- Tone instructions beyond "concise" and "direct" (no "witty," "dry," "reluctant," "cynical," "friendly," "enthusiastic," etc.)
- Nostalgia framing, era references, or cultural positioning
- Emotional affect guidance
- Catchphrases, signature expressions, or verbal tics
- Any instruction about how the agent should feel about anything

The permissible content is limited to:

- "Lead with the answer, not the preamble"
- "Cite files with `path:line` when referencing code"
- "Be concise; skip filler transitions and closing summaries"
- "Don't apologize for errors; state them and move on"
- "When running tools, report only what matters from the result"
- "Read the user's `USER.md` for their context, preferences, and environment"
- "Trust the user's conventions; this configuration was written by them"

#### Scenario: A new user installs with defaults

- **Given**: A user enables the module without overriding any persona options
- **When**: The user runs `companion "explain how TCP works"`
- **Then**: The response is concise and direct
- **And**: The response contains no character voice, catchphrases, or stylized tone
- **And**: The response reads as "a competent local assistant," not as a named persona

#### Scenario: Reviewer checks default content

- **Given**: The file `persona/default/AGENT.md` is inspected
- **When**: The reviewer reads it
- **Then**: The file contains no character name
- **And**: The file contains no tone adjective beyond "concise" or "direct"
- **And**: The file is under 50 lines

### Requirement: Default USER.md Is A Template With Placeholders

The default `USER.md` shipped in `persona/default/USER.md` MUST be a template file with explicit placeholder sections the user is expected to fill in. It MUST NOT contain information about any real user.

The template MUST contain at minimum:

- A header comment explaining the file's purpose and how to customize it
- A "Who I am" section with fields for name, role, and timezone
- A "Machines I work on" section
- A "Communication preferences" section
- A "Things to check before acting" section (optional confirmations for risky operations)
- A "Projects I'm working on" section

All sections MUST use placeholder values (`<your name>`, `<your role>`, etc.) that are obviously not real data.

#### Scenario: Template on fresh install

- **Given**: A new user enables the module without providing `persona.userFile`
- **When**: The wrapper runs for the first time and scaffolds the workspace
- **Then**: A copy of the default `USER.md` template is placed in the workspace
- **And**: The template contains placeholder values the user can see and replace
- **And**: The template explains via comments how to customize it

#### Scenario: User fills in the template

- **Given**: The user has edited their workspace `USER.md` to include real context
- **When**: The user runs `companion "what do you know about me"`
- **Then**: The response reflects the content the user wrote
- **And**: The default placeholder values are not referenced by the agent

### Requirement: Users Override Via Module Options, Not By Editing Package Files

Users who want to replace the default persona MUST do so by setting `services.axios-companion.persona.userFile` and/or `persona.extraFiles`. They MUST NOT be required to edit files inside the Nix store or fork the package.

#### Scenario: User layers a character voice

- **Given**: A user has written their own `./voice.md` with a character persona
- **When**: They set `services.axios-companion.persona.extraFiles = [ ./voice.md ]`
- **And**: They run `home-manager switch` and invoke `companion`
- **Then**: The companion adopts the voice described in `./voice.md`
- **And**: The user did not modify any files in the Nix store

#### Scenario: User completely replaces the user context

- **Given**: A user provides `persona.userFile = ./my-full-context.md`
- **When**: They run `companion`
- **Then**: The default `USER.md` template is not included in the system prompt
- **And**: The user's file is used instead

### Requirement: Default Files Ship As Part Of A Nix Package

The default persona files MUST ship as part of a Nix package (referenced via `persona.basePackage`), not as source paths resolved at evaluation time. This ensures the files are available at runtime regardless of where the flake is consumed from and guarantees reproducibility.

#### Scenario: Flake consumed from GitHub

- **Given**: A user imports the flake via `inputs.axios-companion.url = "github:kcalvelli/axios-companion";`
- **When**: The module evaluates `persona.basePackage`
- **Then**: The default persona files are available from the Nix-built package
- **And**: No source paths need to be resolved outside the Nix store
