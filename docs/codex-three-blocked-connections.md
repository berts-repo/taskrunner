# Three blocked connections, four successful Codex runs

## What an ordinary coding task revealed about background network activity

*Investigation: September 13, 2026, Eastern time. Evidence reviewed and write-up prepared September 14, 2026.*

## Part one: The plain-language story

While testing Codex through Taskrunner, we noticed something unexpected: it repeatedly tried to contact two additional internet destinations. Our network filter blocked those attempts. Codex still completed the work.

Across four runs, the pattern was identical: three blocked attempts each time. Even a request to reply with only “OK,” without reading or changing files, produced the same pattern.

That raises a reasonable question: **what was the software trying to do beyond the task we gave it?**

One possibility is telemetry: information software sends back about its operation, such as usage counts, performance, or errors. Depending on its design, telemetry can also describe the software environment. Whether those details identify a person or device depends on the fields collected and how they are combined.

But our experiment did not reveal the contents of these requests. It established that the attempts happened, that the filter refused them, and that the tested tasks succeeded anyway. It did not establish that the blocked destinations were receiving telemetry, or that Codex was trying to upload information about the device.

### What we actually saw

The two blocked destinations were:

- `sdmntprcentralus.oaiusercontent.com`: one attempt per run.
- `sdmntprsouthcentralus.oaiusercontent.com`: two attempts per run.

Separately, each run had an allowed connection attempt to `ab.chatgpt.com`. The earlier inspection of the installed Codex binary found a metrics endpoint using that host: `https://ab.chatgpt.com/otlp/v1/metrics`.

That is a stronger lead for telemetry than the names of the blocked destinations. However, a permitted connection to a hostname does not show which page or endpoint was requested, what information was sent, or whether delivery succeeded.

### What OpenAI says about telemetry

OpenAI’s current documentation says Codex sends anonymous usage and health metrics by default, describes those metrics as containing no personally identifiable information, and lists context such as authentication mode, model, and Codex version. It distinguishes this collection from optional OpenTelemetry log and trace export. These are the vendor’s descriptions; our network test did not independently verify the transmitted fields. [OpenAI’s telemetry documentation](https://learn.chatgpt.com/docs/config-file/config-advanced#observability-and-telemetry)

So background metrics collection is documented. **The connection between that collection and our three blocked attempts remains unproven.**

### Why “nothing broke” is interesting

Software can keep working when an optional background operation fails. That operation might report metrics, retrieve configuration, fetch an asset, or perform another supporting function.

Our result shows that these destinations were unnecessary for successful completion of the four tasks we tested. It does not show that they are unnecessary for every Codex feature. It also does not distinguish an optional upload from an optional download.

This matters because an application’s visible behavior tells us only part of what it does. A correct answer or completed file does not explain every network request made along the way.

### Was information about the device being stored?

We do not know from this evidence.

There are several separate questions: was device information collected locally, was it prepared for transmission, was it transmitted, and was it retained by a server? Our blocked-connection records do not answer those questions.

The tests ran in a Docker worker container. Any future claim about “my device” would also need to distinguish information visible inside that container from information about the underlying host computer.

The useful privacy question is therefore specific: **what triggers these requests, what information do they carry, and which setting controls them?** Users should be able to understand that without reverse-engineering an application.

## Part two: Technical analysis

### 1. Scope and observations

Taskrunner launched Codex workers and routed their outbound connections through an HTTP egress proxy—a gateway that checks destinations against an allowlist. MCP provided the task interface; the refusals examined here occurred in the worker’s outbound network path, rather than demonstrating a failure of the MCP connection itself.

The relevant path was:

```text
Taskrunner launches Codex in a worker container
                    ↓
Worker requests an outbound tunnel from the egress proxy
                    ↓
Proxy checks destination hostname and port
                    ↓
Allowed: attempt an upstream connection
Refused: return an error without opening the upstream connection
```

The original `events.jsonl` records were recounted for this write-up:

| Archived task | centralus refused | southcentralus refused | ab.chatgpt.com allowed | Outcome |
| --- | ---: | ---: | ---: | --- |
| `task_01m2ep08gd223kp3v4sbdmgaqv` | 1 | 2 | 1 | Completed |
| `task_01m2epk4pkzhma7azfd7xnv68r` | 1 | 2 | 1 | Completed |
| `task_01m2er70fg1b8e977p9w3wq12w` | 1 | 2 | 1 | Completed |
| `task_01m2er7krc1qgwjx2w2z4vbhzh` | 1 | 2 | 1 | Completed |

The abbreviated columns refer to the full `sdmntpr…oaiusercontent.com` hostnames above. All these records concern port 443. The varied prompts included creating a file, replying “OK” without file operations, and listing directory contents.

Four identical observations support repeatability within this setup. They do not establish an unconditional behavior across versions, accounts, configurations, or all prompts. Nor do two attempts to one host prove a retry policy; separate operations could produce the same count.

### 2. Precisely what the proxy blocked

For HTTPS over this proxy, the client first requests a CONNECT tunnel to a hostname and port. In the inspected implementation, an allowlist rejection returns before `net.connect()` opens the upstream socket. The archived investigation showed the same ordering.

Consequently, these refused attempts did not establish TLS sessions with the requested destinations through this proxy. Describing them as “three encrypted connections” is imprecise: they were three refused tunnel requests to port 443.

The records identify destinations and refusal reasons. They do not expose an HTTP method, URL path, request body, or intended upload/download operation.

An earlier explanation claimed no payload could have been assembled before the tunnel existed. That was too strong: an application can prepare or queue a payload before it connects. The defensible conclusion is that the proxy did not forward it to these destinations through the rejected tunnels. This says nothing about data exchanged through other, allowed connections.

### 3. The telemetry evidence is separate

The archived binary inspection found OpenTelemetry-related strings and the literal metrics endpoint `ab.chatgpt.com/otlp/v1/metrics`. The audit records independently show one allow decision for `ab.chatgpt.com` per run.

Together, these observations support a telemetry hypothesis for that allowed host. They do not prove that the observed connection used the embedded endpoint. The proxy logs policy decisions before connection completion, so “allowed” is not evidence of successful delivery or server-side storage.

The earlier investigation also reported blocked attempts near session startup or shutdown in the three runs with detailed timing. Such timing is consistent with background activity, but cannot identify its purpose. Temporal proximity to a metrics connection does not establish a shared payload or subsystem.

### 4. What the binary search could not prove

Searches found no literal `oaiusercontent.com` or `sdmntpr` strings in the inspected executables. That narrows one possibility: the searched text was not present as a directly searchable literal in those files.

It does not prove that OpenAI’s servers supplied the URLs. Runtime construction, encoding, configuration, other resources, or server responses could all explain the absence. Establishing the source requires tracing the actual request construction or the inputs that supplied the destination.

### 5. A controlled follow-up

The next useful test would repeat the same prompts with the same pinned worker image and effective configuration, changing only the analytics setting. OpenAI documents the following control:

```toml
[analytics]
enabled = false
```

The configuration reference describes this as enabling or disabling analytics for the machine/profile; when unset, the client default applies. [OpenAI configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)

The comparison should count the blocked hosts and `ab.chatgpt.com` independently. If disabling analytics removes one pattern while the other remains, that would help separate the behaviors. If both disappear, it would support a relationship without revealing their contents. Neither outcome alone establishes what a server stores.

For attribution, preserve the executable version and hash, image digest, effective settings, authentication mode, prompts, task outcomes, and timestamped proxy records. Then trace the request-producing code or inspect request construction in an isolated test using synthetic inputs. Identifying the method, destination path, and payload schema would answer much more than hostname counts. Retention would still require additional evidence.

## Conclusion

The experiment revealed a repeatable pattern of background connection attempts that could fail without preventing four simple tasks from completing. It also uncovered a separate telemetry lead, and current documentation confirms that Codex has default usage and health metrics collection.

**We have evidence of unexplained connection attempts. We do not have evidence that those blocked requests uploaded or stored device information.**

That distinction makes the investigation more useful: it directs attention toward the missing explanation—what the requests do, what they contain, and how users control them.

## Evidence and provenance

- Original investigation: Claude Code session `709fb542-a491-492a-a1a0-4649e64e0889`, exchanges 35–42, September 14, 2026 UTC (September 13 Eastern).
- Counts: original Taskrunner audit records in `~/.taskrunner/events.jsonl`, filtered by the four task IDs above and recounted September 14. Outcomes and binary inspection: archived investigation tool results.
- Proxy behavior: archived CONNECT-handler inspection and current `docker/egress-proxy/server.cjs`. The current file is corroboration, not a claim that the historical revision was identical in every respect.
- [Earlier published report](https://claude.ai/code/artifact/2bbc7e32-230e-43be-a136-4207683697f1), which may require the owner’s account. This write-up corrects its stronger claims about deterministic retries, binary-string absence, and payload preparation.
- Official documentation linked above was retrieved September 14, 2026. It provides current context, not proof of the exact tested binary’s behavior. No fresh Codex experiment or payload interception was performed for this write-up.
