---
title: Schema
description: Firepit Configuration File Schema
outline: deep
---

# Schema

The JSON schema file is available [here](https://raw.githubusercontent.com/kota65535/firepit/main/schema.json).

Each property section is described as follows:

- **Type**: Type of the property. We use `Array<{item type}>` for arrays, `Map<{key type}, {value type}>` for objects.
- **Required**: Whether the property is required.
- **Default**: Default value of the property.
- **Options**: Available values of the property.
- **Template**: Whether the property is a template string. Only for `string` or types containing strings.
- **Description**: Description of the property.

<!--@include:./schema-body.md-->
