# Introduction

Slumber is a terminal-based HTTP client, built for interacting with REST and other HTTP clients. It has two usage modes: Terminal User Interface (TUI) and Command Line Interface (CLI). The TUI is the most useful, and allows for interactively sending requests and viewing responses. The CLI is useful for sending quick requests and scripting.

<div style="position:relative;padding-bottom:56.25%;height:0;overflow:hidden;max-width:100%;margin:1.5rem 0;border-radius:10px;border:1px solid rgba(128,128,128,0.2);box-shadow:0 4px 20px rgba(0,0,0,0.15);">
  <iframe
    src="https://bisque.cloud/p/github/lucaspickering-slumber"
    title="Slumber: a terminal HTTP client driven by one YAML file — narrated walkthrough"
    style="position:absolute;top:0;left:0;width:100%;height:100%;border:0;"
    loading="lazy"
    allow="autoplay; fullscreen; encrypted-media"
    allowfullscreen></iframe>
</div>

The goal of Slumber is to be **easy to use, configurable, and sharable**. To that end, configuration is defined in a YAML file called the **request collection**. Both usage modes (TUI and CLI) share the same basic configuration, which is called the [request collection](./api/request_collection/index.md).

Check out the [Getting Started guide](./getting_started.md) to try it out, or move onto [Key Concepts](./user_guide/key_concepts.md) to start learning in depth about Slumber.

![Slumber demo](./images/demo.gif)
