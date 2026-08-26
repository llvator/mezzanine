# Commercial licensing

Mezzanine is dual-licensed. You may use it under either:

- the **GNU AGPL v3** ([LICENSE](LICENSE)), at no cost, or
- a **commercial licence**, which removes the AGPL's obligations.

Everything in this repository is available under the AGPL. The commercial
licence exists for organisations that cannot accept those terms.

## Which one do you need?

Most people need neither a decision nor a payment. **Running Mezzanine is not
distribution.** Analysing your own code with the CLI, the VS Code extension,
or the MCP server — including on proprietary code, including across a whole
company — triggers no AGPL obligation at all. Mezzanine reads your source; it does
not link into your product, and your code is not a derivative work of it.

You are likely to want a commercial licence if any of these is true:

- **Your organisation's policy bans the AGPL by name.** This is the common
  case. Several large engineering organisations maintain blanket AGPL bans
  that apply regardless of whether obligations actually arise. If that is
  why you are reading this page, the commercial licence is the shortest path
  to an approved dependency, and the answer is administrative rather than
  technical.
- **You want to offer Mezzanine — or something built on it — to third parties as a
  hosted service.** `mezz serve` hosts several analysed repositories behind an
  HTTP API and a browser UI. Modify it, put it in front of users over a
  network, and AGPL section 13 requires you to offer those users the
  corresponding source of your modified version.
- **You want to redistribute Mezzanine inside a closed-source product**, embed the
  engine in a proprietary tool, or ship a modified build to customers
  without publishing your changes.

The first is routine and the answer is usually yes. The second and third are
negotiated case by case and are not offered as a matter of course — see
[below](#redistribution-embedding-and-hosting-for-third-parties).

If none of those describe you, use the AGPL and ignore this page.

## What the commercial licence grants

Two different things get asked for under one name, and they are priced and
decided differently.

### Internal use

The common request, and the straightforward one. It covers using and
modifying Mezzanine inside your organisation — on any amount of proprietary code,
by any number of your own developers — free of the AGPL's source-disclosure
and network-use obligations, including running `mezz serve` on your own
infrastructure for your own staff.

It does not carry the right to redistribute Mezzanine outside your organisation,
to embed it in something you ship, or to put it in front of third parties as
a service. If what you need is an approved internal dependency, this is the
whole conversation, and it is meant to be a short one.

### Redistribution, embedding, and hosting for third parties

Shipping Mezzanine inside a product, offering it to your customers as a hosted
service, or building a commercial offering on the engine is negotiated
case by case. It is not a standard tier with a price list, and it is not
offered as a matter of course — the terms depend on what is being built and
on what it competes with.

If that is what you have in mind, say so directly in your first message.
It is a different conversation from the one above, and starting it as the
one above wastes both our time.

Support, priority on issues, and roadmap input are separate from all of
this — a licence is a grant of rights, not a support contract.

## Getting in touch

Write to **contact@llvator.com**, or reach us through
[llvator.com](https://llvator.com).

Useful to include, so a first reply can be a concrete one:

- roughly how many developers would use it, and in what capacity
- whether you need redistribution, hosting, or only an approved internal
  dependency
- any deadline you are working to

## For contributors

The dual licence is what makes contribution terms matter here. Mezzanine asks
contributors to sign a [Contributor Licence Agreement](CLA.md) so that
contributed code can be offered under both licences. That is the point of
the CLA, and it is stated plainly there: without it, a contribution could
only ever be distributed under the AGPL, and a commercial licence covering
the whole project would not be possible.
