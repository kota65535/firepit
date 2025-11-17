#!/usr/bin/env node

import fs from 'fs';

if (process.argv.length !== 4) {
    console.error("Usage: schema-to-md.js <input> <output>");
    process.exit(1);
}

const inputPath = process.argv[2];
const outputPath = process.argv[3];

const raw = fs.readFileSync(inputPath, 'utf8');
const schema = JSON.parse(raw);


function refName($ref) {
    // Example: "#/$defs/DependsOnConfigStruct" -> "DependsOnConfigStruct"
    const parts = $ref.split('/');
    return parts[parts.length - 1];
}

function defaultValue(schema) {
    const v = schema.default
    if (typeof v === 'object') {
        return JSON.stringify(v)
    } else {
        return v
    }
}

function typeValue(schema) {
    // - $ref -> [RefName](#refname)
    // - anyOf/oneOf -> join using ' or '
    // - array -> []<item type>
    // - object -> Map<string, <additional properties type>>

    if (schema.$ref) {
        const ref = refName(schema.$ref)
        return `<a href="#${ref.toLowerCase()}">${ref}</a>`;
    }

    if (schema.anyOf || schema.oneOf) {
        let anyOf = schema.anyOf || schema.oneOf
        const parts = anyOf.filter(s => s.type !== 'null').map(s => typeValue(s));
        const uniq = [...new Set(parts)];
        return uniq.join(' | ');
    }

    if (schema.type === 'array') {
        return `Array&lt;${typeValue(schema.items)}&gt;`;
    }

    if (schema.type === 'object') {
        if (typeof schema.additionalProperties === 'boolean') {
            return `Map&lt;string, any&gt;`
        } else {
            return `Map&lt;string, ${typeValue(schema.additionalProperties)}&gt;`
        }
    }

    if (Array.isArray(schema.type)) {
        schema.type = schema.type.filter(s => s !== 'null')
        if (schema.type.length === 1) {
            return schema.type[0];
        }
    }

    return schema.type
}

function renderProperty(schema, name, required) {
    const lines = [];
    if (name != null) {
        lines.push(`### ${name}\n`);
    }

    const type = typeValue(schema)

    lines.push(`- **Type:** <code>${type}</code>`);

    if (required != null) {
        lines.push(`- **Required:** ${required ? 'yes' : 'no'}`);
    }

    if (schema.enum) {
        lines.push(`- **Options:** ${schema.enum.map(v => `\`${v}\``).join(', ')}`);
    }

    if (schema.default != null) {
        lines.push(`- **Default:** \`${defaultValue(schema)}\``);
    }

    if (type.includes("string")) {
        lines.push(`- **Template:** ${schema['x-template'] ? 'yes' : 'no'}`);
    }

    if (schema.description != null) {
        lines.push(`- **Description:** ${schema.description}`);
    }

    return lines.join('\n');
}

function renderProperties(name, schema) {
    const lines = [];
    const props = schema.properties || {};
    const requiredList = schema.required || [];

    if (Object.keys(props).length > 0) {
        lines.push(`## ${name}\n`);
        for (const [propName, propSchema] of Object.entries(props)) {
            let required = requiredList.includes(propName);
            lines.push(renderProperty(propSchema, propName, required));
            lines.push('');
        }
    } else {
        lines.push(`## ${name}\n`);
        lines.push(renderProperty(schema));
        lines.push('');
    }

    return lines.join('\n');
}

function preprocessSchema(schema) {
    schema.$defs.Restart = schema.$defs.Restart.anyOf[0];
    schema.properties.concurrency.default = "Number of available CPU cores";
}

function assembleMarkdown(schema) {
    const parts = [];
    preprocessSchema(schema);
    parts.push(renderProperties("Project", schema));
    for (const [defName, defSchema] of Object.entries(schema.$defs)) {
        parts.push(renderProperties(defName, defSchema));
    }
    return parts.join('\n');
}


const md = assembleMarkdown(schema);

fs.writeFileSync(outputPath, md, 'utf8');
