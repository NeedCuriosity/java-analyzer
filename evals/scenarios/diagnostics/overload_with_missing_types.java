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
// 父类来自缺失的依赖：`super.x` 不能当成 `Object` 上的字段来查
class Child extends org.missing.Base {
    void set(Unit value) {
        super.unitToSkip = value;
    }

    String request(Unit unit) {
        return Requests.create(unit);
    }
}

class Requests {
    String create(String path) { return path; }

    static String create(URI uri) { return uri.toString(); }
}
//? diag only type-mismatch
//? diag only unresolved-field
//? diag only static-context
//? diag only argument-mismatch
