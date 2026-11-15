import java.io.IOException;
import java.io.ObjectInputStream;
import java.util.List;
import java.util.function.BiFunction;

class Main implements java.io.Serializable {
    private int helper(int a, int /*@dead*/dead) { return a; }

    private static String format(String /*@unused_static*/text, int width) {
        return "%" + width + "s";
    }

    // 非私有方法可能被覆写或从别处调用
    int open(int ignoredByApi) { return 1; }

    protected void hook(String event) {}

    // 被方法引用：签名受函数式接口约束
    private int combine(int a, int b) { return a; }

    BiFunction<Integer, Integer, Integer> ref() { return this::combine; }

    // 序列化方法的签名是固定的
    private void readObject(ObjectInputStream in) throws IOException {}

    // 带注解的方法可能是框架的入口
    @Deprecated
    private void annotated(int x) {}

    private void conventions(int unused, int ignored, int _skip) {}

    private void lambdaParams(List<String> names) {
        names.forEach(n -> {});
    }

    private void local(int /*@shadow_only*/value) {
        Runnable r = new Runnable() {
            public void run() {
                int value = 2;
                System.out.println(value);
            }
        };
        r.run();
    }

    private abstract static class Base {
        private Base(int unusedCtorParam) {}
    }

    void use() {
        helper(1, 2);
        format("x", 3);
        annotated(1);
        conventions(1, 2, 3);
        lambdaParams(List.of());
        local(1);
    }
}
//? diag unused-parameter dead
//? diag unused-parameter unused_static
//? diag unused-parameter shadow_only
//? diag only unused-parameter
//? assist remove_unused_parameter dead ; has "private int helper(int a) { return a; }" ; has "helper(1);"
