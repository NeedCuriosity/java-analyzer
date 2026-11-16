//- /p/Util.java
package p;

public class Util {
    public static int /*@twiceDecl*/twice(int x) { return x * 2; }
    public static final String /*@prefixDecl*/PREFIX = "p:";
}
//- /p/Color.java
package p;

public enum Color { /*@redDecl*/RED, GREEN }
//- /p/Tag.java
package p;

public @interface Tag {
    String /*@valueDecl*/value();
    int /*@weightDecl*/weight() default 1;
}
//- /p/Base.java
package p;

public class Base {
    protected /*@baseCtor*/Base(int size) {}
}
//- /p/Point.java
package p;

public record Point(int /*@xDecl*/x, int y) {}
//- /app/Main.java
package app;

import static p.Util./*@twiceImport*/twice;
import static p.Util./*@prefixImport*/PREFIX;

import java.util.function.IntUnaryOperator;
import java.util.function.Supplier;
import p.*;

@Tag(/*@implicitValue*/"main")
class Main extends Base {
    Main() { /*@superCall*/super(3); }

    @Tag(value = "x", /*@weightUse*/weight = 2)
    int run(Color c, Point pt) {
        IntUnaryOperator op = Util::/*@twiceRef*/twice;
        Supplier<Object> s = Main::/*@ctorRef*/new;
        int n = /*@twiceCall*/twice(pt./*@xUse*/x());
        String label = /*@prefixUse*/PREFIX + c;
        switch (c) {
            case /*@redCase*/RED -> n++;
            default -> n--;
        }
        if (c == Color./*@redQualified*/RED) n += op.applyAsInt(1);
        return n + label.length() + s.hashCode();
    }
}
//? refs twiceDecl = twiceImport twiceRef twiceCall
//? refs prefixDecl = prefixImport prefixUse
//? refs redDecl = redCase redQualified
//? refs weightDecl = weightUse
//? refs xDecl = xUse
//? refs baseCtor = superCall
//? rename twiceDecl doubled ; has "import static p.Util.doubled;" ; has "Util::doubled" ; has "int n = doubled(pt.x());"
//? rename redDecl CRIMSON ; has "case CRIMSON ->" ; has "Color.CRIMSON"
//? rename weightDecl priority ; has "priority = 2"
//? diag none
