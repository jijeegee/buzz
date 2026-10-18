import 'package:flutter/material.dart';

import 'my_agent_actions.dart';

/// The small round status light used next to an agent.
class MyAgentStatusDot extends StatelessWidget {
  const MyAgentStatusDot({super.key, required this.status, this.size = 8});

  final MyAgentStatus status;
  final double size;

  @override
  Widget build(BuildContext context) => Container(
    width: size,
    height: size,
    decoration: BoxDecoration(
      shape: BoxShape.circle,
      color: myAgentStatusColor(context, status),
    ),
  );
}
