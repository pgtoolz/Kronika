use std::cmp::Ordering;

pub(crate) fn compare_products(left: &[u128], right: &[u128]) -> Ordering {
    if let (Some(left), Some(right)) = (fixed_product_limbs(left), fixed_product_limbs(right)) {
        left.len.cmp(&right.len).then_with(|| {
            left.digits[..left.len]
                .iter()
                .rev()
                .cmp(right.digits[..right.len].iter().rev())
        })
    } else {
        let left = heap_product_limbs(left);
        let right = heap_product_limbs(right);
        left.len().cmp(&right.len()).then_with(|| left.cmp(&right))
    }
}

struct FixedProduct {
    digits: [u32; 16],
    len: usize,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each cast selects one base-2^32 limb after shifting"
)]
fn fixed_product_limbs(factors: &[u128]) -> Option<FixedProduct> {
    if factors.len() > 4 {
        return None;
    }
    let mut accumulator = FixedProduct {
        digits: [0; 16],
        len: 1,
    };
    accumulator.digits[0] = 1;
    for factor in factors {
        let factor_digits = [
            *factor as u32,
            (*factor >> 32) as u32,
            (*factor >> 64) as u32,
            (*factor >> 96) as u32,
        ];
        let mut result_digits = [0_u32; 16];
        for left_index in 0..accumulator.len {
            let left = accumulator.digits[left_index];
            let mut carry = 0_u64;
            for (right_index, right) in factor_digits.iter().copied().enumerate() {
                let index = left_index + right_index;
                let slot = result_digits.get_mut(index)?;
                let value = u64::from(*slot) + u64::from(left) * u64::from(right) + carry;
                *slot = value as u32;
                carry = value >> 32;
            }
            let mut index = left_index + factor_digits.len();
            while carry > 0 {
                let slot = result_digits.get_mut(index)?;
                let value = u64::from(*slot) + carry;
                *slot = value as u32;
                carry = value >> 32;
                index += 1;
            }
        }
        let mut len = (accumulator.len + factor_digits.len()).min(result_digits.len());
        while len > 1 && result_digits[len - 1] == 0 {
            len -= 1;
        }
        accumulator = FixedProduct {
            digits: result_digits,
            len,
        };
    }
    Some(accumulator)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "each cast selects one base-2^32 limb after shifting"
)]
fn heap_product_limbs(factors: &[u128]) -> Vec<u32> {
    let mut accumulator = vec![1_u32];
    for factor in factors {
        let digits = [
            *factor as u32,
            (*factor >> 32) as u32,
            (*factor >> 64) as u32,
            (*factor >> 96) as u32,
        ];
        let mut multiplied = vec![0_u32; accumulator.len() + digits.len()];
        for (left_index, left) in accumulator.iter().copied().enumerate() {
            let mut carry = 0_u64;
            for (right_index, right) in digits.iter().copied().enumerate() {
                let index = left_index + right_index;
                let value =
                    u64::from(multiplied[index]) + u64::from(left) * u64::from(right) + carry;
                multiplied[index] = value as u32;
                carry = value >> 32;
            }
            let mut index = left_index + digits.len();
            while carry > 0 {
                let value = u64::from(multiplied[index]) + carry;
                multiplied[index] = value as u32;
                carry = value >> 32;
                index += 1;
                if index == multiplied.len() && carry > 0 {
                    multiplied.push(0);
                }
            }
        }
        while multiplied.len() > 1 && multiplied.last() == Some(&0) {
            multiplied.pop();
        }
        accumulator = multiplied;
    }
    accumulator.iter().rev().copied().collect()
}
