import { memo } from 'react';
import {
  Download,
  AlertCircle,
  AlertTriangle,
  Settings,
  ShieldCheck,
  Webhook,
  Mail,
  MessageSquare,
  Layers,
  Activity,
  History,
  Timer,
  Clock,
  Info,
} from 'lucide-react';

export const EventIcon = memo(
  ({ eventType, className }: { eventType: string; className?: string }) => {
    const type = eventType.toLowerCase();
    if (type.includes('download')) return <Download className={className} />;
    if (type.includes('error') || type.includes('fail'))
      return <AlertCircle className={className} />;
    if (type.includes('warning'))
      return <AlertTriangle className={className} />;
    if (type.includes('config') || type.includes('settings'))
      return <Settings className={className} />;
    if (type.includes('auth')) return <ShieldCheck className={className} />;
    if (type.includes('webhook')) return <Webhook className={className} />;
    if (type.includes('email')) return <Mail className={className} />;
    if (type.includes('danmu') || type.includes('chat'))
      return <MessageSquare className={className} />;
    if (type.includes('pipeline')) return <Layers className={className} />;
    if (type.includes('engine')) return <Activity className={className} />;
    if (type.includes('retention')) return <History className={className} />;
    if (type.includes('delay') || type.includes('timer'))
      return <Timer className={className} />;
    if (type.includes('recording') || type.includes('session'))
      return <Clock className={className} />;

    return <Info className={className} />;
  },
);
EventIcon.displayName = 'EventIcon';
