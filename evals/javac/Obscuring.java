import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

/** 名字的遮蔽与同名：变量、类型、方法各有自己的名字空间，内层声明盖住外层。 */
public class Obscuring {
    private int count = 1;
    private String label = "outer";
    static String Name = "field";

    /** 和静态字段同名的成员类型：表达式里字段优先，类型位置只能是类型。 */
    static class Name {
        static String of(String text) {
            return "type:" + text;
        }
    }

    static class Base {
        protected int value = 10;
        static String kind() {
            return "base";
        }

        class Item {
            int weight = 1;
        }
    }

    static class Derived extends Base {
        protected String value = "derived";

        class Item {
            String weight = "heavy";
        }

        int baseValue() {
            return super.value + value.length();
        }

        String itemWeight() {
            Item item = new Item();
            return item.weight;
        }
    }

    /** 方法和字段同名、方法和类同名都是合法的。 */
    static class Counter {
        int total;

        int total() {
            return total;
        }

        Counter Counter() {
            return this;
        }
    }

    /** 类型形参盖住同名的类。 */
    static class Box<Name> {
        Name content;

        Name get() {
            return content;
        }

        <Policy> Policy convert(Function<Name, Policy> mapper) {
            return mapper.apply(content);
        }
    }

    int shadowedByParameter(int count) {
        return count + this.count;
    }

    String shadowedByLocal() {
        String label = "local";
        return label + this.label;
    }

    int fieldWinsInExpression() {
        return Name.length();
    }

    String typeInTypePosition() {
        Name unused = new Name();
        return Obscuring.Name.length() + unused.toString();
    }

    String inaccessibleFieldFallsBackToType() {
        Policy.Statement statement = Policy.Statement.of("Allow");
        return statement.effect + Policy.Version.CURRENT;
    }

    int localNamedLikeClass() {
        String String = "abc";
        Integer Integer = 4;
        return String.length() + Integer.intValue();
    }

    int labelAndVariable() {
        int outer = 0;
        outer:
        for (int i = 0; i < 3; i++) {
            for (int j = 0; j < 3; j++) {
                if (j == 2) {
                    continue outer;
                }
                outer += j;
            }
        }
        return outer;
    }

    int lambdaParameters(List<String> items) {
        Function<String, Integer> length = label -> label.length() + count;
        int sum = 0;
        for (String item : items) {
            sum += length.apply(item);
        }
        return sum;
    }

    int innerShadowsOuter() {
        class Local {
            int count = 5;

            int both() {
                return count + Obscuring.this.count;
            }
        }
        return new Local().both();
    }

    String inherited() {
        Derived derived = new Derived();
        Base base = derived;
        return derived.value + base.value + derived.baseValue() + derived.itemWeight() + Derived.kind();
    }

    int methodsAndFields() {
        Counter counter = new Counter();
        counter.total = 3;
        return counter.total() + counter.Counter().total;
    }

    String typeParameters() {
        Box<String> box = new Box<>();
        box.content = "abc";
        Integer length = box.convert(text -> text.length());
        return box.get() + length;
    }

    public static void main(String[] args) {
        Obscuring o = new Obscuring();
        List<String> items = new ArrayList<>();
        items.add("a");
        System.out.println(o.shadowedByParameter(2) + o.shadowedByLocal() + o.fieldWinsInExpression());
        System.out.println(o.typeInTypePosition() + o.inaccessibleFieldFallsBackToType());
        System.out.println(o.localNamedLikeClass() + o.labelAndVariable() + o.lambdaParameters(items));
        System.out.println(o.innerShadowsOuter() + o.inherited() + o.methodsAndFields());
        System.out.println(o.typeParameters() + new Policy().size() + new Name().hashCode());
    }
}

/**
 * 字段和成员类型同名：类自己的代码里，表达式位置是字段，类型位置是类型；
 * 别的类看不到私有字段，同名的成员类型仍然可用。
 */
class Policy {
    private List<Statement> Statement = new ArrayList<>();
    private String Version = "1";

    static class Statement {
        String effect;

        static Statement of(String effect) {
            Statement statement = new Statement();
            statement.effect = effect;
            return statement;
        }
    }

    static class Version {
        static final String CURRENT = "2012-10-17";
    }

    int size() {
        return Statement.size() + Version.length();
    }
}
