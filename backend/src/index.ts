import express, { Express, Request, Response } from 'express';
import { createServer } from 'http';
import { Server as SocketIOServer } from 'socket.io';
import { WebSocketService } from './services/websocket.service';
import testRoutes from './api/test.js';

const app: Express = express();
const server = createServer(app);
const io = new SocketIOServer(server, {
  cors: {
    origin: process.env.FRONTEND_URL || "http://localhost:5173",
    methods: ["GET", "POST"]
  }
});

const PORT = process.env.PORT ?? 3000;

const wsService = new WebSocketService(io);

app.use(express.json());

app.use('/api/test', testRoutes);

app.get('/health', (_req: Request, res: Response) => {
  res.json({ 
    status: 'ok', 
    message: 'StellarStream Backend is running',
    websocket: true,
    connectedUsers: wsService.getConnectedUsers().length
  });
});

app.get('/ws-status', (_req: Request, res: Response) => {
  res.json({
    connectedUsers: wsService.getConnectedUsers(),
    userConnections: Object.fromEntries(
      wsService.getConnectedUsers().map(addr => [
        addr,
        wsService.getUserSocketCount(addr)
      ])
    )
  });
});

server.listen(PORT, () => {
  console.log(`🚀 Server is running on port ${PORT}`);
  console.log(`🔌 WebSocket server is ready for connections`);
  console.log(`🧪 Test endpoints available at /api/test/*`);
});

export default app;
export { wsService };
