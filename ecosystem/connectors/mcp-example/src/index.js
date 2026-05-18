/**
 * Example MCP Server for CyberClaw
 *
 * 这是一个示例 MCP Server，演示了如何：
 * - 实现 MCP 协议
 * - 提供 Tools 和 Resources
 * - 处理 JSON-RPC 请求
 */

const express = require('express');
const jsonrpc = require('jsonrpc-lite');
const winston = require('winston');

// 配置日志
const logger = winston.createLogger({
  level: 'info',
  format: winston.format.combine(
    winston.format.timestamp(),
    winston.format.json()
  ),
  transports: [
    new winston.transports.Console(),
    new winston.transports.File({ filename: 'mcp-server.log' })
  ]
});

// 创建 Express 应用
const app = express();
app.use(express.json());

// ==================== MCP Tools ====================

const tools = [
  {
    name: 'calculate',
    description: 'Perform mathematical calculations',
    inputSchema: {
      type: 'object',
      properties: {
        operation: {
          type: 'string',
          enum: ['add', 'subtract', 'multiply', 'divide', 'power', 'sqrt'],
          description: 'Mathematical operation to perform'
        },
        a: {
          type: 'number',
          description: 'First operand'
        },
        b: {
          type: 'number',
          description: 'Second operand (not needed for sqrt)'
        }
      },
      required: ['operation', 'a']
    }
  },
  {
    name: 'text_transform',
    description: 'Transform text in various ways',
    inputSchema: {
      type: 'object',
      properties: {
        text: {
          type: 'string',
          description: 'Text to transform'
        },
        transformation: {
          type: 'string',
          enum: ['uppercase', 'lowercase', 'reverse', 'base64_encode', 'base64_decode'],
          description: 'Transformation to apply'
        }
      },
      required: ['text', 'transformation']
    }
  },
  {
    name: 'data_analysis',
    description: 'Analyze data and provide statistics',
    inputSchema: {
      type: 'object',
      properties: {
        data: {
          type: 'array',
          items: { type: 'number' },
          description: 'Array of numbers to analyze'
        },
        operations: {
          type: 'array',
          items: {
            type: 'string',
            enum: ['mean', 'median', 'mode', 'std', 'min', 'max', 'sum']
          },
          description: 'Statistical operations to perform'
        }
      },
      required: ['data', 'operations']
    }
  },
  {
    name: 'http_request',
    description: 'Make HTTP requests',
    inputSchema: {
      type: 'object',
      properties: {
        url: {
          type: 'string',
          format: 'uri',
          description: 'URL to request'
        },
        method: {
          type: 'string',
          enum: ['GET', 'POST', 'PUT', 'DELETE'],
          default: 'GET'
        },
        headers: {
          type: 'object',
          description: 'Request headers'
        },
        body: {
          type: 'object',
          description: 'Request body (for POST/PUT)'
        }
      },
      required: ['url']
    }
  }
];

// ==================== MCP Resources ====================

const resources = [
  {
    uri: 'config://server',
    name: 'Server Configuration',
    mimeType: 'application/json',
    description: 'Current server configuration'
  },
  {
    uri: 'data://sample',
    name: 'Sample Data',
    mimeType: 'application/json',
    description: 'Sample dataset for testing'
  },
  {
    uri: 'template://report',
    name: 'Report Template',
    mimeType: 'text/markdown',
    description: 'Template for generating reports'
  }
];

// ==================== MCP Prompts ====================

const prompts = [
  {
    name: 'analyze_code',
    description: 'Analyze code quality and suggest improvements',
    template: 'Please analyze the following code:\n\n```{{language}}\n{{code}}\n```\n\nFocus on: {{focus_areas}}'
  },
  {
    name: 'generate_docs',
    description: 'Generate documentation',
    template: 'Generate {{doc_type}} documentation for:\n\n{{content}}\n\nInclude: {{sections}}'
  }
];

// ==================== Tool Implementations ====================

async function executeTool(name, args) {
  logger.info(`Executing tool: ${name}`, { args });

  switch (name) {
    case 'calculate':
      return executeCalculate(args);

    case 'text_transform':
      return executeTextTransform(args);

    case 'data_analysis':
      return executeDataAnalysis(args);

    case 'http_request':
      return executeHttpRequest(args);

    default:
      throw new Error(`Unknown tool: ${name}`);
  }
}

function executeCalculate({ operation, a, b }) {
  let result;

  switch (operation) {
    case 'add':
      result = a + b;
      break;
    case 'subtract':
      result = a - b;
      break;
    case 'multiply':
      result = a * b;
      break;
    case 'divide':
      if (b === 0) throw new Error('Division by zero');
      result = a / b;
      break;
    case 'power':
      result = Math.pow(a, b);
      break;
    case 'sqrt':
      if (a < 0) throw new Error('Cannot calculate square root of negative number');
      result = Math.sqrt(a);
      break;
    default:
      throw new Error(`Unknown operation: ${operation}`);
  }

  return {
    result,
    operation,
    inputs: { a, b }
  };
}

function executeTextTransform({ text, transformation }) {
  let result;

  switch (transformation) {
    case 'uppercase':
      result = text.toUpperCase();
      break;
    case 'lowercase':
      result = text.toLowerCase();
      break;
    case 'reverse':
      result = text.split('').reverse().join('');
      break;
    case 'base64_encode':
      result = Buffer.from(text).toString('base64');
      break;
    case 'base64_decode':
      result = Buffer.from(text, 'base64').toString('utf-8');
      break;
    default:
      throw new Error(`Unknown transformation: ${transformation}`);
  }

  return {
    original: text,
    transformed: result,
    transformation
  };
}

function executeDataAnalysis({ data, operations }) {
  const results = {};

  for (const op of operations) {
    switch (op) {
      case 'mean':
        results.mean = data.reduce((a, b) => a + b, 0) / data.length;
        break;
      case 'median':
        const sorted = [...data].sort((a, b) => a - b);
        const mid = Math.floor(sorted.length / 2);
        results.median = sorted.length % 2 !== 0
          ? sorted[mid]
          : (sorted[mid - 1] + sorted[mid]) / 2;
        break;
      case 'mode':
        const frequency = {};
        data.forEach(val => {
          frequency[val] = (frequency[val] || 0) + 1;
        });
        const maxFreq = Math.max(...Object.values(frequency));
        results.mode = Object.keys(frequency)
          .filter(key => frequency[key] === maxFreq)
          .map(Number);
        break;
      case 'std':
        const mean = data.reduce((a, b) => a + b, 0) / data.length;
        const variance = data.reduce((sum, val) => sum + Math.pow(val - mean, 2), 0) / data.length;
        results.std = Math.sqrt(variance);
        break;
      case 'min':
        results.min = Math.min(...data);
        break;
      case 'max':
        results.max = Math.max(...data);
        break;
      case 'sum':
        results.sum = data.reduce((a, b) => a + b, 0);
        break;
    }
  }

  return {
    data_points: data.length,
    results
  };
}

async function executeHttpRequest({ url, method = 'GET', headers = {}, body = null }) {
  const fetch = (await import('node-fetch')).default;

  const options = {
    method,
    headers
  };

  if (body && (method === 'POST' || method === 'PUT')) {
    options.body = JSON.stringify(body);
    options.headers['Content-Type'] = 'application/json';
  }

  const response = await fetch(url, options);
  const responseBody = await response.text();

  let parsedBody;
  try {
    parsedBody = JSON.parse(responseBody);
  } catch {
    parsedBody = responseBody;
  }

  return {
    status: response.status,
    statusText: response.statusText,
    headers: Object.fromEntries(response.headers.entries()),
    body: parsedBody
  };
}

// ==================== Resource Implementations ====================

async function readResource(uri) {
  logger.info(`Reading resource: ${uri}`);

  switch (uri) {
    case 'config://server':
      return {
        version: '1.0.0',
        tools: tools.length,
        resources: resources.length,
        prompts: prompts.length,
        uptime: process.uptime(),
        memory: process.memoryUsage()
      };

    case 'data://sample':
      return {
        users: [
          { id: 1, name: 'Alice', age: 30 },
          { id: 2, name: 'Bob', age: 25 },
          { id: 3, name: 'Charlie', age: 35 }
        ],
        products: [
          { id: 1, name: 'Laptop', price: 999 },
          { id: 2, name: 'Mouse', price: 29 },
          { id: 3, name: 'Keyboard', price: 79 }
        ]
      };

    case 'template://report':
      return `# Report Template

## Summary
{{summary}}

## Data Analysis
{{analysis}}

## Recommendations
{{recommendations}}

## Conclusion
{{conclusion}}

---
Generated: {{timestamp}}
`;

    default:
      throw new Error(`Unknown resource: ${uri}`);
  }
}

// ==================== JSON-RPC Handler ====================

async function handleJsonRpc(request) {
  const { method, params, id } = request;

  try {
    let result;

    switch (method) {
      // Tool methods
      case 'tools/list':
        result = tools;
        break;

      case 'tools/call':
        result = await executeTool(params.name, params.arguments);
        break;

      // Resource methods
      case 'resources/list':
        result = resources;
        break;

      case 'resources/read':
        result = await readResource(params.uri);
        break;

      // Prompt methods
      case 'prompts/list':
        result = prompts;
        break;

      case 'prompts/get':
        const prompt = prompts.find(p => p.name === params.name);
        if (!prompt) throw new Error(`Unknown prompt: ${params.name}`);
        result = prompt;
        break;

      // Server methods
      case 'server/info':
        result = {
          name: 'CyberClaw MCP Example Server',
          version: '1.0.0',
          capabilities: {
            tools: true,
            resources: true,
            prompts: true
          }
        };
        break;

      default:
        throw new Error(`Method not found: ${method}`);
    }

    return jsonrpc.success(id, result);
  } catch (error) {
    logger.error('JSON-RPC error:', error);
    return jsonrpc.error(id, jsonrpc.JsonRpcError.internalError(error.message));
  }
}

// ==================== Routes ====================

// JSON-RPC endpoint
app.post('/rpc', async (req, res) => {
  const request = req.body;
  logger.info('Received JSON-RPC request:', request);

  const response = await handleJsonRpc(request);
  logger.info('Sending JSON-RPC response:', response);

  res.json(response);
});

// Health check
app.get('/health', (req, res) => {
  res.json({
    status: 'healthy',
    timestamp: new Date().toISOString(),
    uptime: process.uptime()
  });
});

// Tool list (REST endpoint for convenience)
app.get('/tools', (req, res) => {
  res.json(tools);
});

// Resource list (REST endpoint for convenience)
app.get('/resources', (req, res) => {
  res.json(resources);
});

// ==================== Server Startup ====================

const PORT = process.env.PORT || 8080;

app.listen(PORT, () => {
  logger.info(`MCP Server started on port ${PORT}`);
  logger.info(`JSON-RPC endpoint: http://localhost:${PORT}/rpc`);
  logger.info(`Health check: http://localhost:${PORT}/health`);
  logger.info(`Available tools: ${tools.map(t => t.name).join(', ')}`);
  logger.info(`Available resources: ${resources.map(r => r.uri).join(', ')}`);
});

// Graceful shutdown
process.on('SIGTERM', () => {
  logger.info('SIGTERM received, shutting down gracefully');
  process.exit(0);
});

process.on('SIGINT', () => {
  logger.info('SIGINT received, shutting down gracefully');
  process.exit(0);
});