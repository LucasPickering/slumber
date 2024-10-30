# Key Concepts

There are a handful of key concepts you need to understand how to effectively configure and use Slumber. You can read about each one in detail on its linked API reference page.

## [Request Collection](../api/request_collection/index.md)

The collection is the main form of configuration. It defines a set of request recipes, which enable Slumber to make requests to your API.

## [Request Recipe](../api/request_collection/request_recipe.md)

A recipe defines which HTTP requests Slumber can make. A recipe generally corresponds to one endpoint on an API, although you can create as many recipes per endpoint as you'd like.

## [Template](./templates.md)

Templates are Slumber's most powerful feature. They allow you to dynamically build URLs, query parameters, request bodies, etc. using predefined _or_ dynamic values.

## [Profile](../api/request_collection/profile.md)

A profile is a set of static template values. A collection can contain a list of profiles, allowing you to quickly switch between different sets of values. This is useful for using different deployment environments, different sets of IDs, etc.
