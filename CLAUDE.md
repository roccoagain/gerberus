# Working on Gerberus

Read [docs/development.md](docs/development.md) first.

## Keep it simple

Each piece of code does one clear thing. Don't add rarely used options, unused flags or hedged wording. A rule may
carry the edge cases real boards need (bulk vs decoupling capacitors, ESD protection inside a chip) when they make
its results more correct; a case that only adds a knob or a guess doesn't belong.
