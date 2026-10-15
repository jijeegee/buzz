import 'package:flutter/material.dart';

import '../theme/theme.dart';

/// Product name shown where the server (community) name used to be.
const appBrandName = 'Kuzz';

/// The app logo in a round badge. Used instead of a community icon when the
/// app runs against its built-in server.
class AppBrandLogo extends StatelessWidget {
  const AppBrandLogo({super.key, this.size = 40});

  final double size;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      label: appBrandName,
      image: true,
      child: Container(
        width: size,
        height: size,
        decoration: BoxDecoration(
          color: context.colors.primaryContainer,
          shape: BoxShape.circle,
        ),
        alignment: Alignment.center,
        padding: EdgeInsets.all(size * 0.2),
        child: Image.asset(
          'assets/images/buzz-icon.png',
          fit: BoxFit.contain,
          excludeFromSemantics: true,
        ),
      ),
    );
  }
}
