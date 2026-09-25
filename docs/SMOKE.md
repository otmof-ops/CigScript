# A field guide to the lexicon

CigScript names its ideas after the parts of a cigarette. This is not a
joke that got out of hand; it is a joke that was carefully kept in hand, so
that every keyword is a picture you already have. Read this once and the
language sticks. (There it is again.)

Serious meaning on the right. Everything on the left is also true.

| word | in the smoking metaphor | in the language |
|---|---|---|
| **roll** | You roll one when you intend to do something with it later. It is yours to fiddle with. | Declare a variable. `roll count = 0` |
| **stick** | A finished cigarette. You do not re-roll a stick; you light it or you leave it. | Declare a constant. `stick limit = 3`. Reassigning one is an error with its own code. |
| **pull** | A drag. You pull on it, you get something back. | Define a function. `pull greet(name) { snuff "hi ${name}" }` |
| **pack** | Where sticks live between uses. Small, portable, hands you one when asked. | An anonymous function. `pack(x) => x * 2` |
| **snuff** | Put it out and walk off with what you got out of it. | Return from a function. `snuff value` |
| **exhale** | What leaves you and enters the room. Everyone can see it. | Print to stdout. `exhale "done", count` |
| **cough** | The body's exception handler. Comes from somewhere deep and interrupts everything. | Raise an error. `cough "no space left"` |
| **ashtray** | Where the mess is supposed to land, so it does not land on the carpet. | Catch an error. `try { } ashtray err { }` |
| **burn** | The only moment anything actually happens. Everything before it was preparation. | The only place side effects are allowed. `burn { fs.rm(path) }` |
| **unlit** | Held in the mouth, never lit. You look like you mean it, and nothing happens. | Record an effect as an intent without executing it. `burn unlit { }` |
| **chain** | Lighting the next one from the last. Not recommended by physicians. Highly recommended by release managers. | Automation: an ordered list of sticks. `chain release { fetch, build, ship }` |
| **light** | Where the chain starts. | Run a chain. `light(release)` |

## The error

Every failed run leads with the same sentence:

```
Don't see any cigarettes.
```

It means what it says. Whatever you asked for, the runtime looked and found
nothing it could light. Below it is the real diagnostic, with a code, a
caret and a fix, because a one-liner is a mood and not a debugger:

```
Don't see any cigarettes.
error[E701 burn]: fs.rm changes the world, so it must be inside a burn block
  --> tidy.cig:14:5
   |
14 |     fs.rm(stale)
   |     ^^^^^^^^^^^^
  = hint: wrap it: burn { fs.rm(...) }
  = explain: cig explain E701
```

## Frequently muttered questions

**Why cigarettes?** Because the thing the language is about, the difference
between having something in your hand and having lit it, is the whole
metaphor, and it is one everybody understands whether or not they have ever
smoked. Preparation is free. Fire is the only irreversible act. Most
scripting languages let you set fire to things in the middle of a sentence.
CigScript makes you say `burn` first, and then it takes a photo.

**Is this healthy?** CigScript contains 0 mg of nicotine, produces no smoke,
and has never been near a lung. The only thing it is bad for is the habit of
running scripts you have not read.

**Does the theme go all the way down?** No, and on purpose. Ten themed words
carry the ideas CigScript adds. `if`, `while`, `for`, `and`, `or`, `not`,
`true`, `false`, `null` are the words you expect, because renaming `if` makes
a language larger without making it better. The 1.x prototypes tried
`filter` for `if` and `lit` for `true`. They did not survive contact with a
reader.

**Can I write a chain that lights itself?** You can. It fails after 32
levels with a message asking whether that was on purpose.

**What do I call a script?** A `.cig` file. What do I call running one?
Lighting it, if you like, or running it, if you are at work.

**Is `unburn` a keyword?** No. Rollback is not something a script decides;
the kernel does it when a run fails, and you do it later with
`cig unburn <id>`. A script that could un-burn its own past would be a script
you could not read.

**What is the collective noun for chains?** A cough.
