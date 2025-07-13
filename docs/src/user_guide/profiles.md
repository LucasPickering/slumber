# Profiles

A profile is a set of values accessible to templates that you can easily switch between. In the TUI, this is via the Profiles modal (hotkey `p` by default). In the CLI, use the `--profile` (or `-p`) flag.

The canonical use case for profiles is to switch between different API hosts. Here's what that looks like:

```yaml
profiles:
  local:
    data:
      host: http://localhost:5000
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish:
    type: request
    method: GET
    url: "{{ host }}/fishes"
```

But profiles aren't restricted to setting the API host. The `data` field can hold whatever fields you want. Here's an example of setting your API user via profiles:

```yaml
profiles:
  user1:
    data:
      username: user1
  user2:
    data:
      username: user2

requests:
  list_fish:
    type: request
    method: GET
    url: "https://myfishes.fish/fishes"
    authentication:
      type: basic
      username: "{{ username }}"
      password: "{{ file('./password.txt') }}"
```

## Dynamic profile values

Fun fact: profile values _are_ templates! This means you can put dynamic values in your profiles and they'll be rendered automatically with no extra effort. For example, if you want a profile for each user that you may log in as, plus an additional profile that lets you prompt for a username:

```yaml
profiles:
  user1:
    data:
      username: user1
  user2:
    data:
      username: user2
  user_prompt:
    data:
      username: "{{ prompt(message='Username') }}"

requests:
  list_fish:
    type: request
    method: GET
    url: "https://myfishes.fish/fishes"
    authentication:
      type: basic
      username: "{{ username }}"
      password: "{{ file('./password.txt') }}"
```

When you send the `list_fish` command with the `user_prompt` profile selected, it will prompt you to enter a username, then user that value for `{{ username }}`.

This feature can also be used to [deduplicate common template expressions](./templates/examples.md#deduplicating-template-expressions).

## Template caching

In the example above, we saw how a profile field can contain a dynamic template. These dynamic profile fields are automatically cached within the scope of a single request. This means if you use the same field multiple times in a request, **the template will only be rendered once**. Here's an extension of the above example:

```yaml
profiles:
  user1:
    data:
      username: user1
  user2:
    data:
      username: user2
  user_prompt:
    data:
      username: "{{ prompt(message='Username') }}"

requests:
  list_fish:
    type: request
    method: GET
    url: "https://myfishes.fish/fishes"
    authentication:
      type: basic
      username: "{{ username }}"
      password: "{{ file('./password.txt') }}"
    query:
      username: "{{ username }}"
```

In this example, two different fields in the request (`authentication.username` and `query.username`) both reference the `username` profile field. But the corresponding template `{{ prompt(message='Username') }}` **is only rendered once**. That means you'll only be prompted once for a username, and entered value will be used for both instances of `{{ username }}`.

This caching applies only when a single profile field is referenced multiple times within a single request. If you send the same request a second time, you will be prompted again for a username.
