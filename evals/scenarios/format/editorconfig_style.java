//- /.editorconfig
root = true

[*]
indent_style = space
indent_size = 2
insert_final_newline = true

[*.java]
ij_continuation_indent_size = 4
ij_java_indent_case_from_switch = false
ij_java_keep_blank_lines_in_code = 1
ij_java_keep_blank_lines_before_right_brace = 0
ij_java_blank_lines_after_class_header = 1
ij_java_space_before_if_parentheses = false
ij_java_space_within_brackets = true
ij_java_class_brace_style = next_line
ij_java_method_brace_style = end_of_line
//- /Styled.java
class Styled {
    private int count;
    int total(int[] values, int extra) 
    {
        int sum = 0;



        for (int value : values) {
            switch (value) {
                case 0:
                    break;
                default:
                    sum += value
                        + extra;
            }
        }
        if (sum > values[0]) {
            return sum;
        }

        return values[0];

    }
}
//- /Styled.java formatted
class Styled
{

  private int count;

  int total(int[] values, int extra) {
    int sum = 0;

    for (int value : values) {
      switch (value) {
      case 0:
        break;
      default:
        sum += value
            + extra;
      }
    }
    if(sum > values[ 0 ]) {
      return sum;
    }

    return values[ 0 ];
  }
}
