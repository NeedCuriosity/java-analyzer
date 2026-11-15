//- /app/Parent.java
package app;

public sealed interface Parent permits Allowed, Base {}
//- /app/Allowed.java
package app;

public final class Allowed implements Parent {}
//- /app/Base.java
package app;

public sealed abstract class Base implements Parent permits Leaf {}
//- /app/Leaf.java
package app;

public non-sealed class Leaf extends Base {}
//- /app/Forbidden.java
package app;

public final class /*@forbidden*/Forbidden implements Parent {}
//- /app/Sneaky.java
package app;

public final class /*@sneaky*/Sneaky extends Base {}
//- /app/Free.java
package app;

public class Free extends Leaf {}
//- /app/Shapes.java
package app;

sealed interface Shape {}
final class Circle implements Shape {}
record Square(int side) implements Shape {}
enum Unit implements Shape { ONE }
//- /app/Use.java
package app;

class Use {
    Parent anon() { return new /*@anon*/Parent() {}; }
}
//? diag sealed-subtype forbidden
//? diag sealed-subtype sneaky
//? diag sealed-subtype anon
//? diag exact
