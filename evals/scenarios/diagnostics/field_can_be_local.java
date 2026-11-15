import java.io.Serializable;
import java.util.List;

class Main {
    private int /*@temp*/temp;
    private String state;
    private int counter;
    private int seeded = 5;
    private int two;
    private int conditional;

    int compute(List<Integer> items) {
        this.temp = items.size();
        int result = temp * 2;
        temp = temp + 1;
        return result + this.temp;
    }

    void remember(String s) {
        state = s;
    }

    String recall() {
        return state;
    }

    int increment() {
        counter++;
        return counter;
    }

    int seed() {
        seeded = seeded + 1;
        return seeded;
    }

    int branch(boolean flag) {
        if (flag) {
            conditional = 1;
        }
        return conditional;
    }

    int other(Main m) {
        two = 2;
        return m.two;
    }
}

class Saved implements Serializable {
    private int value;

    int calc() {
        value = 3;
        return value;
    }
}
//? diag field-can-be-local temp
//? diag only field-can-be-local
//? assist convert_field_to_local temp ; has "int compute(List<Integer> items) {\n        int temp = items.size();\n        int result = temp * 2;\n        temp = temp + 1;\n        return result + temp;" ; lacks "private int temp;"
