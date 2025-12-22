# Build stage
FROM node:22-alpine AS builder

WORKDIR /app

# Copy package files
COPY package.json package-lock.json ./

# Install dependencies
RUN npm ci

# Copy source files
COPY . .

# Build the application
RUN npm run build

# Runtime stage
FROM node:22-alpine

WORKDIR /app

# Copy built assets
COPY --from=builder /app/.output ./.output

# Default env vars
ENV PORT=80
ENV HOST=0.0.0.0
ENV NODE_ENV=production

# Expose port
EXPOSE 80

# Start the server
CMD ["node", ".output/server/index.mjs"]
