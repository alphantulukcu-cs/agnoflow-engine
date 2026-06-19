# Autoexec Nodes Implementation Guide

## Overview

Autoexec nodes are automatic workflow execution steps that don't require human interaction. They execute when their `when` condition is true and automatically advance the workflow based on their results. This system supports three types of autoexec nodes: **REST**, **SQL**, and **CALC**.

## Architecture

### Module Structure

```
crates/wfe/src/autoexec/
├── mod.rs              # Main module, AutoexecExecutor orchestrator
├── error.rs            # Error types for autoexec operations
├── schema.rs           # Input/output schema validation and mapping
├── rest.rs             # REST HTTP executor
├── sql.rs              # SQL database executor
├── calc.rs             # Expression calculator executor
└── tests.rs            # Comprehensive unit tests
```

### Key Components

1. **AutoexecExecutor** - Routes execution to the appropriate handler based on autoexec type
2. **SchemaValidator** - Validates and maps input/output parameters according to JSON schemas
3. **RestExecutor** - Handles HTTP requests (GET, POST, PUT, DELETE)
4. **SqlExecutor** - Executes SQL queries against databases
5. **CalcExecutor** - Evaluates mathematical and boolean expressions

## Autoexec Types

### 1. REST Autoexec

Makes HTTP requests and maps responses back to workflow context.

**Configuration Schema:**
```json
{
  "type": "rest",
  "method": "GET|POST|PUT|DELETE",
  "url": "https://api.example.com/endpoint",
  "params": {
    "param_key": { "ref": "$ctx.context.field" }
  },
  "result": {
    "output_field": "$.response.path.to.value"
  }
}
```

**Example:**
```json
{
  "id": "auto_tc_verify",
  "when": "$wfah.some(# .action == \"Başvur\")",
  "autoexec": {
    "type": "rest",
    "method": "GET",
    "url": "https://api.internal/tc-verify",
    "params": {
      "tckn": { "ref": "$ctx.basvuran.user" }
    },
    "result": {
      "tc_gecerli": "$.valid"
    }
  },
  "wfes_effects": {
    "set": {
      "tc_gecerli": "$exec.result.tc_gecerli"
    }
  },
  "wft": {
    "c_a": []
  }
}
```

**Parameter Mapping:**
- `{ "ref": "$ctx.field.path" }` - Reference to context value
- `{ "ctx": "field.path" }` - Shorthand context reference
- Literal values - Any JSON literal

**Output Mapping (JSONPath):**
- `"$.field"` - Root-level field
- `"$.data.user.id"` - Nested path
- `"$.array[0]"` - Array element

### 2. SQL Autoexec

Executes SQL queries and returns results as JSON.

**Configuration Schema:**
```json
{
  "type": "sql",
  "database_type": "postgres|mysql|sqlite",
  "query": "SELECT * FROM users WHERE id = :user_id",
  "params": {
    "user_id": { "ref": "$ctx.user.id" }
  },
  "result": {
    "user_name": "$.name",
    "user_email": "$.email"
  }
}
```

**Example:**
```json
{
  "id": "auto_fetch_user",
  "when": "$user_id != null",
  "autoexec": {
    "type": "sql",
    "database_type": "postgres",
    "query": "SELECT name, email, status FROM users WHERE id = :user_id",
    "params": {
      "user_id": { "ref": "$ctx.user.id" }
    },
    "result": {
      "user_name": "$.name",
      "user_email": "$.email",
      "user_status": "$.status"
    }
  },
  "wfes_effects": {
    "set": {
      "user_name": "$exec.result.user_name",
      "user_email": "$exec.result.user_email"
    }
  },
  "wft": {
    "c_a": []
  }
}
```

**Supported Databases:**
- PostgreSQL (fully implemented)
- MySQL (framework ready)
- SQLite (framework ready)

**Query Parameters:**
- Use `:param_name` syntax for parameter substitution
- Parameters are automatically escaped

### 3. CALC Autoexec

Evaluates mathematical and boolean expressions using ZEN syntax.

**Configuration Schema:**
```json
{
  "type": "calc",
  "expressions": {
    "output_field": "ctx.field1 + ctx.field2",
    "bool_field": "ctx.amount > 1000 and ctx.status == 'active'"
  }
}
```

**Example:**
```json
{
  "id": "auto_calc_eligibility",
  "when": "$kredi_notu > 0",
  "autoexec": {
    "type": "calc",
    "expressions": {
      "limit_uygun": "ctx.kredi_notu >= 600 and ctx.miktar <= 500000"
    }
  },
  "wfes_effects": {
    "set": {
      "limit_uygun": "$exec.result.limit_uygun"
    }
  },
  "wft": {
    "c_a": [
      {
        "c_orgu": { "from": "$ctx._step_Başvur.actor.orgu", "traverse": "parent" },
        "c_r": ["subeMuduru"]
      }
    ]
  }
}
```

**Expression Syntax (ZEN-based):**
- Variables: `ctx.fieldname` (converted to `$fieldname`)
- Operators: `+`, `-`, `*`, `/`, `>`, `<`, `>=`, `<=`, `==`, `!=`
- Logical: `and`, `or`, `!` (converted to `&&`, `||`)
- Parentheses: `(expr1 + expr2) * expr3`

## Integration with Effects

### Accessing Autoexec Results

Autoexec results are automatically merged into the context under `_exec.result.*` and can be referenced in `wfes_effects`:

```json
"wfes_effects": {
  "set": {
    "status": "$exec.result.status",
    "user_data": "$exec.result.user_info"
  }
}
```

### Effect Value Syntax

Special syntax for referencing autoexec results:

```
$exec.result.field_name       - Direct field reference
$exec.result.nested.path      - Nested field reference
```

This syntax is supported in all effect value contexts (set/append).

## Execution Flow

1. **Evaluation** - Check `when` condition against current WFES
2. **Execution** - Execute autoexec node based on type
3. **Result Mapping** - Extract and map results using output schema
4. **Effects** - Apply `wfes_effects` with access to autoexec results
5. **Persistence** - Save updated context
6. **WFT Resolution** - Determine next candidates or terminal state

## Error Handling

### Error Types

- **SchemaValidationFailed** - Input/output schema validation error
- **RestRequestFailed** - HTTP request failed
- **SqlExecutionFailed** - SQL query execution failed
- **DatabaseConnectionFailed** - Database connection unavailable
- **ExpressionEvaluationFailed** - CALC expression evaluation error
- **InvalidConfiguration** - Missing or malformed autoexec configuration
- **ParameterMappingFailed** - Failed to resolve parameter value
- **ResultMappingFailed** - Failed to extract result value

### Error Propagation

Autoexec errors are logged and returned as `EngineError::Autoexec`, which prevents further workflow execution until resolved.

## Best Practices

### 1. Parameter Mapping

Always validate that context fields exist before referencing:
```json
{
  "params": {
    "user_id": { "ref": "$ctx.user.id" }
  }
}
```

### 2. Output Schema

Design output mappings to handle both single and multiple results:
```json
{
  "result": {
    "primary_id": "$.id",
    "primary_name": "$.name"
  }
}
```

### 3. Conditional Routing

Use CALC nodes to determine routing before expensive REST/SQL calls:
```json
{
  "when": "$amount > 1000",
  "autoexec": { "type": "calc", ... }
}
```

### 4. Idempotency

Make autoexec nodes idempotent where possible - they may be retried:
- Use unique identifiers in requests
- Include deduplication in SQL queries
- Avoid side effects in CALC expressions

### 5. Monitoring

Log autoexec execution for debugging:
- All autoexec results are captured in WFAH
- Failed autoexec returns detailed error messages
- Context changes are persisted atomically

## Configuration Examples

### Real-world: Credit Application Workflow

```json
{
  "id": "kredi-basvuru",
  "transitions": [
    {
      "id": "auto_tc_verify",
      "when": "some($wfah, # .action == \"Başvur\")",
      "autoexec": {
        "type": "rest",
        "method": "GET",
        "url": "https://identity-service/verify/{{tc_number}}",
        "params": {
          "tc_number": { "ref": "$ctx.applicant.tc_number" }
        },
        "result": {
          "verified": "$.is_valid"
        }
      },
      "wfes_effects": {
        "set": {
          "tc_verified": "$exec.result.verified"
        }
      }
    },
    {
      "id": "auto_credit_check",
      "when": "$tc_verified == true",
      "autoexec": {
        "type": "sql",
        "database_type": "postgres",
        "query": "SELECT credit_score, limit FROM credit_bureau WHERE person_id = :pid",
        "params": {
          "pid": { "ref": "$ctx.applicant.person_id" }
        },
        "result": {
          "score": "$.credit_score",
          "available_limit": "$.limit"
        }
      },
      "wfes_effects": {
        "set": {
          "credit_score": "$exec.result.score"
        }
      }
    },
    {
      "id": "auto_eligibility",
      "when": "$credit_score > 0",
      "autoexec": {
        "type": "calc",
        "expressions": {
          "is_eligible": "ctx.credit_score >= 600 and ctx.requested_amount <= ctx.available_limit"
        }
      },
      "wfes_effects": {
        "set": {
          "eligible": "$exec.result.is_eligible"
        }
      }
    }
  ]
}
```

## Testing

The autoexec system includes comprehensive unit tests:

```bash
# Run all autoexec tests
cargo test -p wf-wfe --lib autoexec

# Run specific test suite
cargo test -p wf-wfe --lib autoexec::tests::schema_validator
```

Test coverage includes:
- Schema validation and mapping
- Input parameter resolution
- Output extraction via JSONPath
- Type conversions
- Error handling

## Future Enhancements

- [ ] Support for GraphQL queries (GraphQL executor)
- [ ] Native support for MySQL and SQLite
- [ ] Request signing (OAuth, AWS Signature)
- [ ] Response caching and retry mechanisms
- [ ] Template expressions in URLs and queries
- [ ] Multi-step sequential autoexec chains
- [ ] Parallel autoexec execution with join semantics
