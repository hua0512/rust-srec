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

# Runtime stage - serve with nginx
FROM nginx:alpine

# Copy built assets to nginx
COPY --from=builder /app/.output/public /usr/share/nginx/html

# Copy nginx configuration template
COPY nginx.conf.template /etc/nginx/templates/default.conf.template

# Default backend URL (can be overridden via environment variable)
ENV BACKEND_URL=http://rust-srec:8080

EXPOSE 80

# nginx:alpine automatically runs envsubst on templates in /etc/nginx/templates/
CMD ["nginx", "-g", "daemon off;"]
