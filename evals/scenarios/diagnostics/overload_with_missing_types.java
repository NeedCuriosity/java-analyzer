import java.net.URI;
import org.missing.Unit;

// 一个重载的形参类型来自缺失的依赖：实参类型已知时选形参类型已知且适用的那个重载
class Util {
    static String toUri(Unit unit) { return ""; }

    static URI toUri(String text) { return null; }

    static String fix(URI uri) { return uri.toString(); }

    void use(String text) {
        URI uri = toUri(text);
        String fixed = fix(toUri(text));
        System.out.println(uri + fixed);
    }

    // 实参的类型也不知道：两个重载都可能，结果类型当作未知，不报类型不匹配
    void unknownArgument(Unit unit) {
        String name = toUri(unit);
        URI other = toUri(Missing.make());
        System.out.println(name + other);
    }
}
//? diag only type-mismatch
//? diag only argument-mismatch
