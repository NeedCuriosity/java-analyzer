import java.util.List;

interface Description {
    String name();
}

interface FilterableList<T, S extends FilterableList<T, S>> extends List<T> {
    S filter(String name);
}

// 无界通配符 `?` 捕获后的上界是类型参数声明的上界 `Description`
interface ParameterList<T extends Description> extends FilterableList<T, ParameterList<T>> {}

interface Method {
    ParameterList<?> getParameters();
}

class Use {
    void run(Method method) {
        for (Description parameter : method.getParameters()) {
            parameter.name();
        }
        Description first = method.getParameters().get(0);
        var /*@second*/second = method.getParameters().filter("x").get(1);
        String name = second.name();
    }
}
//? hover second ~ "Description second"
//? diag none
